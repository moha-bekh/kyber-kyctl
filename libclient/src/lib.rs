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
use tokio::sync::mpsc::{self, error::TrySendError, Sender};

pub use crate::error::{Error, Result};
use enums::Message;
pub use enums::{ControlPlaneCloseReason, DataPlaneCloseReason, Display, Event, StopReason};
use event_manager::EventManager;
pub use event_manager::EventSink;
use host_events::HostEventReceiver;
pub use host_events::{StreamerPipelineEventType, StreamerPipelineStage, StreamerPipelineType};
use http::HttpClient;
pub use platform::{ConnectConfig, VideoPlayerConfig};
pub(crate) use platform::{ConnectionParams, InputPipeline, MainLoopEngine};

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
    pub(crate) fn send(&self, msg: Message) -> Result<()> {
        match self.sender.try_send(msg) {
            Ok(_) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(Error::ChannelSend),
            Err(TrySendError::Full(_)) => {
                warn!("MessageProxy: Channel full: message dropped");

                Ok(())
            }
        }
    }

    pub(crate) async fn send_async(&self, msg: Message) -> Result<()> {
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
        let ret = host_service::stop_session(&self.http_client).await;
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

    async fn stop_hostevents(&mut self) -> Result<()> {
        let Some(host_events) = self.host_events.take() else {
            return Ok(());
        };

        let ret = host_events.stop().await;
        match ret {
            Ok(_) => {
                info!("HostEventHandler stopped");
            }
            Err(err) => {
                warn!("Failed to stop HostEventHandler: {err:?}");
            }
        }

        Ok(())
    }

    async fn stop_backend(&mut self) -> Result<()> {
        if let Some(mut backend) = self.backend.take() {
            backend.stop().await?;
        }

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stop backend");
        self.stop_backend().await?;

        info!("Request to stop host session");
        let ret = host_service::stop_session(&self.http_client).await;
        if let Err(err) = ret {
            warn!(
                "Stop session rejected by controller ({:?}). Stop client anyway.",
                err
            );
        } else {
            info!("Host session stopped");
        }

        self.stop_hostevents().await?;

        Ok(())
    }

    async fn start_streaming_session(
        &mut self,
        streaming_config: &StreamingSessionConfig,
    ) -> Result<()> {
        let msg_proxy = self.create_message_proxy();

        info!("Start streaming session");
        streaming_config.print();

        match &streaming_config.protocol {
            #[cfg(feature = "websocket")]
            StreamingProtocol::Websocket => {
                let config = ws_backend::WsBackendConfig {
                    http_client: &self.http_client,
                    streaming_config,
                    msg_proxy,
                };

                let backend = Box::new(ws_backend::WsBackend::new(config).await?);
                self.backend = Some(backend);
            }
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(kymux_config) => {
                let config = kymux_backend::KymuxBackendConfig {
                    conn_params: &self.conn_params,
                    http_client: &self.http_client,
                    streaming_config,
                    kymux_config: kymux_config.clone(),
                    msg_proxy,
                };

                let backend = Box::new(kymux_backend::KymuxBackend::new(config).await?);
                self.backend = Some(backend);
            }
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => {
                let config = rtp_backend::RtpBackendConfig {
                    conn_params: &self.conn_params,
                    http_client: &self.http_client,
                    streaming_config,
                    msg_proxy,
                };

                let backend = Box::new(rtp_backend::RtpBackend::new(config).await?);
                self.backend = Some(backend);
            }
        };

        Ok(())
    }

    async fn stop_streaming_session(&mut self) -> Result<()> {
        self.stop_backend().await?;

        info!("Request to stop streaming");
        let ret = host_service::stop_streaming(&self.http_client).await;
        if let Err(err) = ret {
            warn!(
                "Stop streaming rejected by controller ({:?}). Stop client anyway.",
                err
            );
        } else {
            info!("Streaming stopped");
        }

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
                info!("Disconnect requested by user");
                let ret = self.stop().await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::Requested,
                });

                ret
            }
            Message::ControlPlaneClosed { reason } => {
                warn!("ControlPlaneClosed. Reason: {reason:?}");

                let ret_backend = self.stop_backend().await;
                let ret_hostevents = self.stop_hostevents().await;
                let ret = ret_backend.and(ret_hostevents);

                let reason = match reason {
                    ControlPlaneCloseReason::Eof => StopReason::ControlPlaneClosed,
                    ControlPlaneCloseReason::Closed { .. } => StopReason::ControlPlaneClosed,
                    ControlPlaneCloseReason::Timeout => StopReason::ControlPlaneTimeout,
                };

                self.event_manager.send_event(Event::Stopped { reason });

                ret
            }
            Message::DataPlaneClosed { reason } => {
                warn!("DataPlaneClosed. Reason: {reason:?}");
                self.event_manager.send_event(Event::DataPlaneClosed);

                Ok(())
            }
            Message::StartStreamingSession { config } => {
                let ret = self.start_streaming_session(config).await;
                let (event, stop) = match ret {
                    Ok(()) => (Event::StreamingStarted, false),
                    Err(Error::DataPlaneConnectFailed) => {
                        warn!("Dataplane connect failed. Stop client");
                        (Event::DataPlaneConnectFailed, true)
                    }
                    Err(err) => {
                        warn!("Streaming start failed: {err:?}. Stop client");
                        (Event::StreamingStartFailed, true)
                    }
                };

                if stop {
                    let ret = self.stop().await;
                    if let Err(err) = ret {
                        warn!("Fail to stop client: {err:?}");
                    }
                }

                self.event_manager.send_event(event);

                Ok(())
            }
            Message::StopStreamingSession => {
                self.stop_streaming_session().await?;

                self.event_manager.send_event(Event::StreamingStopped);

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

                self.event_manager.send_event(Event::InputServiceConnected);

                Ok(())
            }
            Message::InputServiceDisconnected => {
                info!("Input service disconnected");

                if let Some(backend) = self.backend.as_mut() {
                    backend.input_service_disconnected().await?;
                }

                self.event_manager
                    .send_event(Event::InputServiceDisconnected);

                Ok(())
            }
            Message::DisplayListUpdated { displays } => {
                info!("Display list updated: {displays:?}");

                self.event_manager.send_event(Event::DisplayListUpdated {
                    displays: displays.clone(),
                });

                Ok(())
            }
            Message::StreamerPipelineEvent(event) => {
                self.event_manager.send_event(Event::StreamerPipelineEvent {
                    type_: event.type_,
                    stage: event.stage,
                    event: event.event,
                });

                Ok(())
            }
            Message::VideoDecodeError => {
                let ret = self.stop().await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::DecodeError,
                });

                ret
            }
            Message::AudioDecodeError => Ok(()),
            #[cfg(feature = "web")]
            Message::CanvasResized {
                width,
                height,
                frame_coded_width,
                frame_coded_height,
            } => {
                self.event_manager.send_event(Event::CanvasResized {
                    width: *width,
                    height: *height,
                    frame_coded_width: *frame_coded_width,
                    frame_coded_height: *frame_coded_height,
                });

                Ok(())
            }
        }
    }
}

#[cfg(feature = "kymux")]
pub use kymux_backend::{KymuxConfig, KymuxTransportBackend};

#[cfg(feature = "kymux")]
pub use kymux::{AudioProtocol, VideoProtocol};

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
    pub display_id: u32,
    pub codec: VideoCodec,
    pub yuv444: bool,
    pub intra_refresh: Option<bool>,
    pub player: VideoPlayerConfig,
}

#[derive(Clone)]
pub struct InputSessionConfig {
    pub pipeline: InputPipeline,
    pub gamepad_force_xbox360: Option<bool>,
}

#[derive(Clone)]
pub struct StreamingSessionConfig {
    pub protocol: StreamingProtocol,
    pub bitrate: u32,
    pub video: Vec<VideoSessionConfig>,
    pub audio: bool,
    pub input: Option<InputSessionConfig>,
}

impl StreamingSessionConfig {
    pub fn print(&self) {
        info!("StreamingSessionConfig");

        match &self.protocol {
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => {
                info!("\tprotocol: RTP");
            }
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(config) => {
                info!("\tprotocol: Kymux");
                info!("\t\tbackend: {:?}", config.backend);
                info!("\t\tvideo_protocol: {:?}", config.video_protocol);
                info!("\t\taudio_protocol: {:?}", config.audio_protocol);
            }
            #[cfg(feature = "websocket")]
            StreamingProtocol::Websocket => {
                info!("\tprotocol: Websocket");
            }
        }

        info!("\tbitrate: {}", self.bitrate);

        if !self.video.is_empty() {
            for (idx, config) in self.video.iter().enumerate() {
                info!("\tVideo session {idx}");
                info!("\t\tdisplay_id: {}", config.display_id);
                info!("\t\tcodec: {:?}", config.codec);
                info!("\t\tyuv444: {}", config.yuv444);
                info!("\t\tintra_refresh: {:?}", config.intra_refresh);
            }
        } else {
            info!("\tvideo: false");
        };

        info!("\taudio: {}", self.audio);

        if let Some(input) = &self.input {
            info!("\tinput: true");
            info!(
                "\t\tgamepad_force_xbox360: {:?}",
                input.gamepad_force_xbox360
            );
        } else {
            info!("\tinput: false");
        }
    }
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

struct MainLoop {
    receiver: mpsc::Receiver<Message>,
    handler: Handler,
}

impl MainLoop {
    fn new(receiver: mpsc::Receiver<Message>, handler: Handler) -> Self {
        Self { receiver, handler }
    }

    async fn run(mut self) {
        loop {
            let msg = self.receiver.recv().await;
            if let Some(msg) = msg.as_ref() {
                let ret = self.handler.handle_message(msg).await;
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
}

pub struct Client {
    event_manager: EventManager,
    sender: Sender<Message>,

    mainloop_engine: MainLoopEngine,
}

impl Client {
    pub fn new(connect_config: ConnectConfig) -> Result<Self> {
        let event_manager = EventManager::new();

        let (sender, receiver) = mpsc::channel(32);
        let event_manager_handler = event_manager.clone();
        let sender_handler = sender.clone();

        let handler = Handler::new(connect_config, event_manager_handler, sender_handler)?;
        let mainloop = MainLoop::new(receiver, handler);

        let mainloop_engine = MainLoopEngine::new(mainloop);

        Ok(Self {
            event_manager,
            sender,
            mainloop_engine,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn stop(self) -> Result<()> {
        self.mainloop_engine.stop().await;

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn stop(&self) -> Result<()> {
        self.mainloop_engine.stop()
    }

    pub fn connect(&self) -> Result<()> {
        self.send_message(Message::Connect)?;
        Ok(())
    }

    pub fn disconnect(&self) -> Result<()> {
        self.send_message(Message::Disconnect)?;
        Ok(())
    }

    pub fn start_streaming(&self, config: StreamingSessionConfig) -> Result<()> {
        self.send_message(Message::StartStreamingSession { config })?;
        Ok(())
    }

    pub fn stop_streaming(&self) -> Result<()> {
        self.send_message(Message::StopStreamingSession)?;
        Ok(())
    }

    fn send_message(&self, msg: Message) -> Result<()> {
        match self.sender.try_send(msg) {
            Ok(_) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(Error::ChannelSend),
            Err(TrySendError::Full(_)) => {
                warn!("MessageProxy: Channel full: message dropped");

                Ok(())
            }
        }
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
