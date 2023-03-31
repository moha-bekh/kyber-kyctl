mod enums;
mod error;
pub mod event_manager;
mod host_events;
mod host_service;
pub(crate) mod http;
pub mod input_service;
mod kymux_backend;
mod rtp_backend;
mod tls;
mod utils;
pub mod window_handle;

#[cfg(any(cargo_c, feature = "capi"))]
pub mod capi;

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;

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
use window_handle::WindowHandle;

pub(crate) const STREAMER_PORT: u16 = 8080;

enum Player {
    Rtp(rtp_backend::Player),
    Kymux(kymux_backend::Player),
}

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

struct StartStreamResult {
    player: Option<Player>,
    input_pipeline: Option<Pipeline>,
    input_service: Option<input_service::Client>,
}

struct Handler {
    connect_config: ConnectConfig,
    streaming_config: StreamingSessionConfig,
    tls_config: tls::Config,
    streamer_addr: SocketAddr,
    http_client: HttpClient,
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,

    kymux: Option<kymux::Connection>,
    host_events: Option<HostEventReceiver>,
    player: Option<Player>,

    input_service: Option<input_service::Client>,
    input_pipeline: Option<Pipeline>,

    displays: Vec<Display>,
}

impl Handler {
    fn new(
        connect_config: ConnectConfig,
        streaming_config: StreamingSessionConfig,
        event_manager: Arc<Mutex<EventManager>>,
        sender: Sender<Message>,
    ) -> Result<Self> {
        let tls_config = tls::load_tls_certificate(&connect_config.tls_cert)?;
        let streamer_addr = resolve_streamer_host(&connect_config.ip, connect_config.port)?;

        let http_client = HttpClient::new(&connect_config.tls_host, &streamer_addr, &tls_config)?;

        Ok(Self {
            connect_config,
            streaming_config,
            tls_config,
            streamer_addr,
            http_client,
            event_manager,
            sender,
            kymux: None,
            host_events: None,
            player: None,
            input_service: None,
            input_pipeline: None,
            displays: vec![],
        })
    }

    fn create_message_proxy(&self) -> MessageProxy {
        MessageProxy {
            sender: self.sender.clone(),
        }
    }

    async fn start(&mut self) -> Result<Vec<Display>> {
        // Start Kymux
        let kymux = kymux_backend::start_kymux(
            &self.streamer_addr,
            &self.connect_config.tls_host,
            &self.tls_config,
            &self.http_client,
        )
        .await?;

        self.kymux = Some(kymux);

        // Connect to websocket
        let host_events = HostEventReceiver::new(
            self.streamer_addr,
            self.tls_config.clone(),
            self.connect_config.tls_host.clone(),
            self.create_message_proxy(),
        )
        .await?;

        self.host_events = Some(host_events);

        // Enumerate displays
        host_service::enumerate_displays(&self.http_client).await
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stop requested");

        match self.player.take() {
            Some(Player::Kymux(mut player)) => {
                info!("Stop player");
                player.stop_async().await?;
                info!("Player stopped");
            }
            Some(Player::Rtp(mut player)) => {
                info!("Stop player");
                player.stop_async().await?;
                info!("Player stopped");
            }
            None => {}
        }

        if let Some(input_service) = self.input_service.as_mut() {
            info!("Stop input service");
            input_service.stop().await?;
            info!("Input service stopped");
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

        if let Some(mut kymux) = self.kymux.take() {
            let ret = kymux.stop().await;
            match ret {
                Ok(_) => {
                    info!("Kymux stopped");
                }
                Err(err) => {
                    warn!("Failed to stop kymux: {err:?}");
                }
            }
        }

        Ok(())
    }

    async fn start_streaming_session(&mut self) -> Result<()> {
        let msg_proxy = self.create_message_proxy();

        let result = match self.streaming_config.protocol {
            StreamingProtocol::Kymux => {
                let Some(kymux) = self.kymux.as_mut() else {
                    error!("Got start Kymux session, but kymux isn't started");
                    return Err(Error::KymuxNotStarted);
                };

                kymux_backend::start(
                    &self.http_client,
                    &self.displays,
                    &self.streaming_config,
                    kymux,
                    msg_proxy,
                )
                .await?
            }
            StreamingProtocol::Rtp => {
                rtp_backend::start(
                    &self.streamer_addr,
                    &self.http_client,
                    &self.displays,
                    &self.streaming_config,
                    msg_proxy,
                )
                .await?
            }
        };

        self.player = result.player;
        self.input_pipeline = result.input_pipeline;
        self.input_service = result.input_service;

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

                let input_pipeline = self.input_pipeline.as_mut().ok_or(Error::NoInputPipeline)?;
                let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

                input_pipeline.plug_network(&input_service.get_tx_consumer())?;
                input_pipeline.start()?;

                Ok(())
            }
            Message::InputServiceDisconnected => {
                info!("Input service disconnected");

                let input_pipeline = self.input_pipeline.as_mut().ok_or(Error::NoInputPipeline)?;
                input_pipeline.stop()?;

                self.input_service = None;

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
#[repr(C)]
pub enum StreamingProtocol {
    Rtp,
    Kymux,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[repr(C)]
pub enum VideoCodec {
    H264,
    HEVC,
    AV1,
}

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub ip: String,
    pub port: u16,
    pub tls_host: String,
    pub tls_cert: String,
}

#[derive(Clone, Debug)]
pub struct VideoSessionConfig {
    pub display_index: usize,
    pub window_handle: WindowHandle,
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
    ) -> Result<Self> {
        let event_manager = Arc::new(Mutex::new(EventManager::new()));

        let (sender, mut receiver) = mpsc::channel(32);
        let event_manager_handler = event_manager.clone();
        let sender_handler = sender.clone();

        let mut handler = Handler::new(
            connect_config,
            streaming_config,
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
