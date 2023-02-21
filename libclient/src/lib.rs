mod enums;
mod error;
pub mod event_manager;
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

pub use enums::Event;

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info, warn};
use tokio::runtime;
use tokio::sync::mpsc::{self, Sender};

use libkynput_rs::Pipeline;

use crate::error::{Error, Result};
use enums::Message;
use event_manager::EventManager;
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

struct Handler {
    config: ClientConfig,
    tls_config: tls::Config,
    streamer_addr: SocketAddr,
    http_client: HttpClient,
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,

    kymux: Option<kymux::Connection>,
    player: Option<Player>,
    input_service: Option<input_service::Client>,
}

impl Handler {
    fn new(
        config: ClientConfig,
        event_manager: Arc<Mutex<EventManager>>,
        sender: Sender<Message>,
    ) -> Result<Self> {
        let tls_config = tls::load_tls_certificate(&config.streamer_cert)?;
        let streamer_addr = resolve_streamer_host(&config.streamer_ip, config.streamer_port)?;

        let http_client = HttpClient::new(&config.streamer_tls_host, &streamer_addr, &tls_config)?;

        Ok(Self {
            config,
            tls_config,
            streamer_addr,
            http_client,
            event_manager,
            sender,
            kymux: None,
            player: None,
            input_service: None,
        })
    }

    fn create_message_proxy(&self) -> MessageProxy {
        MessageProxy {
            sender: self.sender.clone(),
        }
    }

    async fn start(&mut self) -> Result<()> {
        match self.config.protocol {
            StreamingProtocol::Kymux => {
                let result = kymux_backend::start(
                    &self.config,
                    &self.streamer_addr,
                    &self.tls_config,
                    &self.http_client,
                    self.create_message_proxy(),
                )
                .await?;

                self.kymux = Some(result.kymux);
                self.player = Some(Player::Kymux(result.player));
                self.input_service = result.input_service;
            }
            StreamingProtocol::Rtp => {
                let result = rtp_backend::start(
                    &self.config,
                    &self.streamer_addr,
                    &self.http_client,
                    self.create_message_proxy(),
                )
                .await?;

                self.player = Some(Player::Rtp(result.player));
                self.input_service = result.input_service;
            }
        }

        Ok(())
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

        let mut event_manager = self.event_manager.lock()?;
        event_manager.send_event(Event::Stopped);

        Ok(())
    }

    async fn handle_message(&mut self, msg: &Message) -> Result<()> {
        match msg {
            Message::Start => self.start().await,
            Message::Stop => self.stop().await,
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

                let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;
                self.config
                    .input_pipeline
                    .plug_network(&input_service.get_tx_consumer())?;

                self.config.input_pipeline.start()?;

                Ok(())
            }
            Message::InputServiceDisconnected => {
                info!("Input service disconnected");

                self.config.input_pipeline.stop()?;
                self.input_service = None;

                Ok(())
            }
        }
    }
}

#[derive(Clone)]
#[repr(C)]
pub enum StreamingProtocol {
    Rtp,
    Kymux,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub enum VideoCodec {
    H264,
    HEVC,
    AV1,
}

#[derive(Clone)]
pub struct ClientConfig {
    pub streamer_tls_host: String,
    pub streamer_ip: String,
    pub streamer_port: u16,
    pub streamer_cert: String,
    pub protocol: StreamingProtocol,
    pub window_handle: WindowHandle,
    pub input_pipeline: Pipeline,
    pub video_codec: VideoCodec,
    pub yuv444: bool,
    pub inputs: bool,
    pub audio: bool,
}

pub struct Client {
    event_manager: Arc<Mutex<EventManager>>,
    sender: Sender<Message>,
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        let event_manager = Arc::new(Mutex::new(EventManager::new()));

        let (sender, mut receiver) = mpsc::channel(32);
        let event_manager_handler = event_manager.clone();
        let sender_handler = sender.clone();

        thread::spawn(move || {
            let rt = runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("EventLoop")
                .enable_io()
                .enable_time()
                .build()
                .unwrap();

            rt.block_on(async move {
                let mut handler =
                    Handler::new(config, event_manager_handler, sender_handler).unwrap();

                loop {
                    let msg = receiver.recv().await;
                    if let Some(msg) = msg.as_ref() {
                        let ret = handler.handle_message(msg).await;
                        if let Err(err) = ret {
                            info!(
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

        Self {
            event_manager,
            sender,
        }
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
