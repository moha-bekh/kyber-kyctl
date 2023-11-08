mod enums;
mod error;
pub mod event_manager;
mod host_events;
mod host_service;
pub(crate) mod http;
pub mod input_service;
mod kymux_backend;
mod player;
mod tls;
pub mod window_handle;

#[cfg(any(cargo_c, feature = "capi"))]
pub mod capi;

#[cfg(feature = "rtp")]
mod rtp_backend;

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;

use async_trait::async_trait;
use log::{error, info, warn};
use serde::Serialize;
use tokio::runtime;
use tokio::sync::mpsc::{self, Sender};

use libkynput_rs::Pipeline;

use crate::error::{Error, Result};
use enums::{Display, Event, Message};
use event_manager::EventManager;
use host_events::HostEventReceiver;
use http::HttpClient;
use kymux_backend::KymuxConfig;
use window_handle::WindowHandle;

pub(crate) const STREAMER_PORT: u16 = 8080;

#[derive(Clone)]
pub struct MessageProxy {
    sender: Sender<Message>,
}

impl MessageProxy {
    pub fn send(&self, msg: Message) -> Result<()> {
        self.sender.blocking_send(msg)?;
        Ok(())
    }

    pub async fn send_async(&self, msg: Message) -> Result<()> {
        self.sender.send(msg).await?;
        Ok(())
    }
}

#[async_trait]
trait BackendDriver {
    async fn input_service_connected(&mut self) -> Result<()>;

    async fn input_service_disconnected(&mut self) -> Result<()>;

    async fn stop(&mut self) -> Result<()>;
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) addr: std::net::SocketAddr,
    pub(crate) tls_host: String,
    pub(crate) tls_config: tls::Config,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        let addr = Self::resolve_streamer_host(&connect_config.host, connect_config.port)?;

        let tls_config = tls::load_tls_config(&connect_config.tls_root_cert)?;

        Ok(Self {
            host: connect_config.host,
            port: connect_config.port,
            addr,
            tls_host: connect_config.tls_host.clone(),
            tls_config,
        })
    }

    fn resolve_streamer_host(host: &str, port: u16) -> Result<SocketAddr> {
        let str_addr = format!("{host}:{port}");
        let mut addrs = str_addr.to_socket_addrs().map_err(|err| {
            error!("Fail to resolve host {host}: {err:?}");
            Error::FailToResolveAddress {
                addr: str_addr.clone(),
            }
        })?;

        let ipv4_addr = addrs.find(|addr| matches!(addr, SocketAddr::V4(_)));

        ipv4_addr.ok_or_else(|| {
            error!("Fail to find an IPv4 address for {host}");
            Error::FailToResolveAddress { addr: str_addr }
        })
    }
}

fn get_display_id<'a>(displays: &'a Vec<Display>, config: &VideoSessionConfig) -> &'a Display {
    if config.display_index < displays.len() {
        &displays[config.display_index]
    } else {
        warn!(
            "Display index {} is out of range, fallback to first screen",
            config.display_index
        );

        &displays[0]
    }
}

struct Handler {
    conn_params: ConnectionParams,
    streaming_config: StreamingSessionConfig,
    player_config: PlayerConfig,
    http_client: HttpClient,
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,

    host_events: Option<HostEventReceiver>,

    displays: Vec<Display>,

    backend: Option<Box<dyn BackendDriver + Send>>,
}

impl Handler {
    fn new(
        connect_config: ConnectConfig,
        streaming_config: StreamingSessionConfig,
        player_config: PlayerConfig,
        event_manager: Arc<Mutex<EventManager>>,
        sender: Sender<Message>,
    ) -> Result<Self> {
        let conn_params = ConnectionParams::new(connect_config)?;
        let http_client = HttpClient::new(conn_params.clone())?;

        Ok(Self {
            conn_params,
            streaming_config,
            player_config,
            http_client,
            event_manager,
            sender,
            host_events: None,
            displays: vec![],
            backend: None,
        })
    }

    fn create_message_proxy(&self) -> MessageProxy {
        MessageProxy {
            sender: self.sender.clone(),
        }
    }

    async fn start(&mut self) -> Result<Vec<Display>> {
        // Connect to websocket
        let host_events =
            HostEventReceiver::new(&self.conn_params, self.create_message_proxy()).await?;

        self.host_events = Some(host_events);

        // Enumerate displays
        host_service::enumerate_displays(&self.http_client).await
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stop requested");

        if let Some(mut backend) = self.backend.take() {
            backend.stop().await?;
        }

        info!("Request to stop streaming services");
        let ret = host_service::stop_stream(&self.http_client).await;
        if let Err(err) = ret {
            warn!(
                "Stop stream rejected by controller ({:?}). Stop client anyway.",
                err
            );
        } else {
            info!("Host service stopped");
        }

        if let Some(host_events) = self.host_events.take() {
            let ret = host_events.stop().await;
            match ret {
                Ok(_) => {
                    info!("HostEventHandler stopped");
                }
                Err(err) => {
                    warn!("Failed to stop HostEventHandler: {err:?}");
                }
            }
        }

        Ok(())
    }

    async fn start_streaming_session(&mut self) -> Result<()> {
        let msg_proxy = self.create_message_proxy();

        match &self.streaming_config.protocol {
            StreamingProtocol::Kymux(kymux_config) => {
                let backend = Box::new(
                    kymux_backend::KymuxBackend::new(
                        &self.conn_params,
                        &self.http_client,
                        &self.displays,
                        &self.player_config,
                        &self.streaming_config,
                        kymux_config.clone(),
                        msg_proxy,
                    )
                    .await?,
                );

                self.backend = Some(backend);
            }
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => {
                let backend = Box::new(
                    rtp_backend::RtpBackend::new(
                        &self.conn_params,
                        &self.http_client,
                        &self.displays,
                        &self.player_config,
                        &self.streaming_config,
                        msg_proxy,
                    )
                    .await?,
                );

                self.backend = Some(backend);
            }
        };

        Ok(())
    }

    async fn handle_message(&mut self, msg: &Message) -> Result<()> {
        match msg {
            Message::Start => {
                // Connect to Kymux, Websocket, and enumerate displays
                self.displays = self.start().await?;

                // Start streaming session
                self.start_streaming_session().await?;

                let mut event_manager = self.event_manager.lock()?;
                event_manager.send_event(Event::Started {
                    displays: self.displays.clone(),
                });

                Ok(())
            }
            Message::Stop => {
                let ret = self.stop().await;

                let mut event_manager = self.event_manager.lock()?;
                event_manager.send_event(Event::Stopped);

                ret
            }
            Message::PlayerStarted => {
                info!("Player started");
                Ok(())
            }
            Message::PlayerStopped => {
                info!("Player stopped");
                Ok(())
            }
            Message::InputServiceConnected => {
                info!("Input service connected");

                if let Some(backend) = self.backend.as_mut() {
                    backend.input_service_connected().await?;
                }

                Ok(())
            }
            Message::InputServiceDisconnected => {
                info!("Input service disconnected");

                if let Some(backend) = self.backend.as_mut() {
                    backend.input_service_disconnected().await?;
                }

                Ok(())
            }
            Message::DisplayListUpdated { displays } => {
                let mut event_manager = self.event_manager.lock()?;

                event_manager.send_event(Event::DisplayListUpdated {
                    displays: displays.clone(),
                });

                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum StreamingProtocol {
    #[cfg(feature = "rtp")]
    Rtp,
    Kymux(KymuxConfig),
}

#[derive(Clone, Copy, Debug, Serialize)]
#[repr(C)]
pub enum VideoCodec {
    H264,
    HEVC,
    AV1,
}

#[derive(Clone, Debug, Serialize)]
pub enum CursorFormat {
    Raw,
    Png,
}

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub tls_host: String,
    pub tls_root_cert: String,
}

#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub window_handle: Option<WindowHandle>,
    pub software_decode: bool,
}

#[derive(Clone, Debug)]
pub struct VideoSessionConfig {
    pub display_index: usize,
    pub codec: VideoCodec,
    pub yuv444: bool,
}

#[derive(Clone, Debug)]
pub struct InputSessionConfig {
    pub pipeline: Pipeline,
}

#[derive(Clone, Debug)]
pub struct StreamingSessionConfig {
    pub protocol: StreamingProtocol,
    pub bitrate: u32,
    pub video: Option<VideoSessionConfig>,
    pub audio: bool,
    pub input: Option<InputSessionConfig>,
}

pub struct Client {
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,
}

impl Client {
    pub fn new(
        connect_config: ConnectConfig,
        streaming_config: StreamingSessionConfig,
        player_config: PlayerConfig,
    ) -> Result<Self> {
        let event_manager = Arc::new(Mutex::new(EventManager::new()));

        let (sender, mut receiver) = mpsc::channel(32);
        let event_manager_handler = event_manager.clone();
        let sender_handler = sender.clone();

        let mut handler = Handler::new(
            connect_config,
            streaming_config,
            player_config,
            event_manager_handler,
            sender_handler,
        )?;

        thread::spawn(move || {
            let rt = runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("EventLoop")
                .enable_io()
                .enable_time()
                .build()
                .unwrap();

            rt.block_on(async move {
                loop {
                    let msg = receiver.recv().await;
                    if let Some(msg) = msg.as_ref() {
                        let ret = handler.handle_message(msg).await;
                        if let Err(err) = ret {
                            warn!(
                                "Processing of message {:?} failed with error {:?}",
                                msg, err
                            );
                        }
                    } else {
                        break;
                    }
                }
            });
        });

        Ok(Self {
            event_manager,
            sender,
        })
    }

    pub fn start(&self) -> Result<()> {
        self.sender.blocking_send(Message::Start)?;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        self.sender.blocking_send(Message::Stop)?;
        Ok(())
    }

    pub fn get_event_manager(&self) -> Arc<Mutex<EventManager>> {
        self.event_manager.clone()
    }
}
