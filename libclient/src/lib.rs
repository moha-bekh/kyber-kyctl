mod enums;
mod error;
pub mod event_manager;
mod host_events;
mod host_service;
pub(crate) mod http;

#[cfg_attr(feature = "desktop", path = "./platform/desktop/mod.rs")]
#[cfg_attr(feature = "web", path = "./platform/web/mod.rs")]
mod platform;

#[cfg(any(cargo_c, feature = "capi"))]
pub mod capi;

#[cfg(feature = "kymux")]
mod kymux_backend;

#[cfg(feature = "rtp")]
mod rtp_backend;

#[cfg(feature = "websocket")]
mod ws_backend;

use async_trait::async_trait;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, Sender};

use crate::error::{Error, Result};
use enums::Message;
pub use enums::{ControlPlaneCloseReason, Display, Event, StopReason};
use event_manager::EventManager;
pub use event_manager::EventSink;
use host_events::HostEventReceiver;
use http::HttpClient;
pub use platform::{ConnectConfig, PlayerConfig};
pub(crate) use platform::{ConnectionParams, InputPipeline};

#[allow(dead_code)]
pub(crate) const STREAMER_PORT: u16 = 8080;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ControlPlaneCapabilities {
    pub(crate) keepintvl: u64,
    pub(crate) keepcnt: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Capabilities {
    pub(crate) control_plane: ControlPlaneCapabilities,
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

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
trait BackendDriver {
    async fn input_service_connected(&mut self) -> Result<()>;

    async fn input_service_disconnected(&mut self) -> Result<()>;

    async fn stop(&mut self) -> Result<()>;
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
    http_client: HttpClient,
    event_manager: EventManager,
    sender: Sender<Message>,

    host_events: Option<HostEventReceiver>,

    displays: Vec<Display>,

    #[cfg(target_arch = "wasm32")]
    backend: Option<Box<dyn BackendDriver>>,
    #[cfg(not(target_arch = "wasm32"))]
    backend: Option<Box<dyn BackendDriver + Send>>,
}

impl Handler {
    fn new(
        connect_config: ConnectConfig,
        event_manager: EventManager,
        sender: Sender<Message>,
    ) -> Result<Self> {
        let conn_params = ConnectionParams::new(connect_config)?;
        let http_client = HttpClient::new(conn_params.clone())?;

        Ok(Self {
            conn_params,
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

    async fn connect(&mut self) -> Result<()> {
        info!("Get Host capabilities");
        let capabilities = host_service::capabilities(&self.http_client).await?;

        // Kick the previously connected user
        info!("Stop any existing stream");
        let ret = host_service::stop_stream(&self.http_client).await;
        if let Err(err) = ret {
            info!("Stop stream result: {err:?}");
        }

        // Connect to websocket
        info!("Connect to Control Plane");
        let host_events = HostEventReceiver::new(
            &self.conn_params,
            &capabilities.control_plane,
            self.create_message_proxy(),
        )
        .await?;

        self.host_events = Some(host_events);

        // Enumerate displays
        info!("Enumerate displays");
        self.displays = host_service::enumerate_displays(&self.http_client).await?;
        info!("Got displays: {:?}", self.displays);

        Ok(())
    }

    async fn stop_backend(&mut self) -> Result<()> {
        if let Some(mut backend) = self.backend.take() {
            backend.stop().await?;
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stop requested");

        self.stop_backend().await?;

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

    async fn start_streaming_session(
        &mut self,
        streaming_config: &StreamingSessionConfig,
        player_config: &PlayerConfig,
    ) -> Result<()> {
        let msg_proxy = self.create_message_proxy();

        match &streaming_config.protocol {
            #[cfg(feature = "websocket")]
            StreamingProtocol::Websocket => {
                let backend = Box::new(
                    ws_backend::WsBackend::new(
                        &self.conn_params,
                        &self.http_client,
                        &self.displays,
                        player_config,
                        streaming_config,
                        msg_proxy,
                    )
                    .await?,
                );

                self.backend = Some(backend);
            }
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(kymux_config) => {
                let backend = Box::new(
                    kymux_backend::KymuxBackend::new(
                        &self.conn_params,
                        &self.http_client,
                        &self.displays,
                        player_config,
                        streaming_config,
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
                        player_config,
                        streaming_config,
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
            Message::Connect => {
                let event = match self.connect().await {
                    Ok(()) => Event::ControlPlaneConnected {
                        displays: self.displays.clone(),
                    },
                    Err(_) => Event::ControlPlaneConnectFailed,
                };

                self.event_manager.send_event(event);

                Ok(())
            }
            Message::Disconnect => {
                let ret = self.stop().await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::Requested,
                });

                ret
            }
            Message::ControlPlaneClosed { reason } => {
                warn!("ControlPlaneClosed. Reason: {reason:?}");

                let ret = self.stop_backend().await;

                let reason = match reason {
                    ControlPlaneCloseReason::Eof => StopReason::ControlPlaneClosed,
                    ControlPlaneCloseReason::Closed { .. } => StopReason::ControlPlaneClosed,
                    ControlPlaneCloseReason::Timeout => StopReason::ControlPlaneTimeout,
                };

                self.event_manager.send_event(Event::Stopped { reason });

                ret
            }
            Message::StartStreamingSession {
                streaming_config,
                player_config,
            } => {
                self.start_streaming_session(streaming_config, player_config)
                    .await?;

                self.event_manager.send_event(Event::StreamingStarted);

                Ok(())
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
                info!("Display list updated: {displays:?}");

                self.event_manager.send_event(Event::DisplayListUpdated {
                    displays: displays.clone(),
                });

                Ok(())
            }
            Message::DecodeError => {
                let ret = self.stop().await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::DecodeError,
                });

                ret
            }
            #[cfg(feature = "web")]
            Message::CanvasResized { width, height } => {
                self.event_manager.send_event(Event::CanvasResized {
                    width: *width,
                    height: *height,
                });

                Ok(())
            }
        }
    }
}

#[cfg(feature = "kymux")]
pub use kymux_backend::{KymuxConfig, KymuxTransportBackend};

#[cfg(feature = "kymux")]
pub use kymux::VideoProtocol;

#[derive(Clone, Debug)]
pub enum StreamingProtocol {
    #[cfg(feature = "rtp")]
    Rtp,
    #[cfg(feature = "kymux")]
    Kymux(KymuxConfig),
    #[cfg(feature = "websocket")]
    Websocket,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
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
pub struct VideoSessionConfig {
    pub display_index: usize,
    pub codec: VideoCodec,
    pub yuv444: bool,
}

#[derive(Clone)]
pub struct InputSessionConfig {
    pub pipeline: InputPipeline,
}

#[derive(Clone)]
pub struct StreamingSessionConfig {
    pub protocol: StreamingProtocol,
    pub bitrate: u32,
    pub video: Option<VideoSessionConfig>,
    pub audio: bool,
    pub input: Option<InputSessionConfig>,
}

impl std::fmt::Debug for StreamingSessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingSessionConfig")
            .field("protocol", &self.protocol)
            .field("bitrate", &self.bitrate)
            .field("video", &self.video)
            .field("audio", &self.audio)
            .field("input", &self.input.is_some())
            .finish()
    }
}

pub struct Client {
    event_manager: EventManager,
    sender: Sender<Message>,
}

impl Client {
    pub fn new(connect_config: ConnectConfig) -> Result<Self> {
        let event_manager = EventManager::new();

        let (sender, receiver) = mpsc::channel(32);
        let event_manager_handler = event_manager.clone();
        let sender_handler = sender.clone();

        let handler = Handler::new(connect_config, event_manager_handler, sender_handler)?;

        #[cfg(feature = "desktop")]
        {
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("EventLoop")
                    .enable_io()
                    .enable_time()
                    .build()
                    .unwrap();

                rt.block_on(Self::mainloop(receiver, handler));
            });
        }

        #[cfg(feature = "web")]
        {
            wasm_bindgen_futures::spawn_local(Self::mainloop(receiver, handler));
        }

        Ok(Self {
            event_manager,
            sender,
        })
    }

    async fn mainloop(mut receiver: mpsc::Receiver<Message>, mut handler: Handler) {
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
    }

    pub fn connect(&self) -> Result<()> {
        self.sender.blocking_send(Message::Connect)?;
        Ok(())
    }

    pub fn disconnect(&self) -> Result<()> {
        self.sender.blocking_send(Message::Disconnect)?;
        Ok(())
    }

    pub fn start_streaming(
        &self,
        streaming_config: StreamingSessionConfig,
        player_config: PlayerConfig,
    ) -> Result<()> {
        self.sender.blocking_send(Message::StartStreamingSession {
            streaming_config,
            player_config,
        })?;
        Ok(())
    }

    pub fn get_event_manager(&self) -> EventManager {
        self.event_manager.clone()
    }

    #[cfg(feature = "js-player")]
    pub async fn supported_video_codecs(
        force_software: bool,
    ) -> Vec<platform::player::VideoDecoderConfig> {
        platform::player::supported_video_codecs(force_software).await
    }
}
