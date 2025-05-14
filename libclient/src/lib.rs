mod enums;
mod error;
pub mod event_manager;
mod host_events;
mod host_service;
pub(crate) mod http;
mod message_bus;
pub mod metrics;
pub(crate) mod runtime;

#[cfg_attr(feature = "desktop", path = "./platform/desktop/mod.rs")]
#[cfg_attr(feature = "web", path = "./platform/web/mod.rs")]
mod platform;

#[cfg(feature = "capi")]
pub mod capi;

#[cfg(feature = "kymux")]
mod kymux_backend;

#[cfg(feature = "rtp")]
mod rtp_backend;

#[cfg(feature = "websocket")]
mod ws_backend;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::{info, warn};
use serde::{Deserialize, Serialize};

pub use crate::error::{Error, Result};
use enums::Message;
pub use enums::{ControlPlaneCloseReason, DataPlaneCloseReason, Display, Event, StopReason};
use event_manager::EventManager;
pub use event_manager::EventSink;
use host_events::HostEventReceiver;
pub use host_events::{StreamerPipelineEventType, StreamerPipelineStage, StreamerPipelineType};
use http::HttpClient;
pub use libclient_common::metrics::Metric;
pub(crate) use message_bus::{MessageReceiver, MessageSender};
use metrics::{MetricsCollector, MetricsProducer};
pub use platform::{ConnectConfig, VideoPlayerConfig};
pub(crate) use platform::{ConnectionParams, InputPipeline, MainLoopEngine};

pub fn init_crypto() {
    #[cfg(feature = "desktop")]
    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap();
}

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

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
trait BackendDriver {
    fn audio_set_volume(&self, volume: f32) -> Result<()>;

    fn audio_set_mute(&self, mute: bool) -> Result<()>;

    async fn input_service_connected(&mut self) -> Result<()>;

    async fn input_service_disconnected(&mut self) -> Result<()>;

    async fn stop(&mut self) -> Result<()>;
}

struct Handler {
    conn_params: ConnectionParams,
    http_client: HttpClient,
    event_manager: EventManager,
    message_sender: MessageSender,
    metrics: Option<MetricsProducer>,

    host_events: Option<HostEventReceiver>,

    state: Arc<Mutex<State>>,
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
        message_sender: MessageSender,
        metrics: Option<MetricsProducer>,
        state: Arc<Mutex<State>>,
    ) -> Result<Self> {
        let conn_params = ConnectionParams::new(connect_config)?;
        let http_client = HttpClient::new(conn_params.clone())?;

        Ok(Self {
            conn_params,
            http_client,
            event_manager,
            message_sender,
            metrics,
            host_events: None,
            state,
            displays: vec![],
            backend: None,
        })
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
            self.message_sender.clone(),
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

    async fn disconnect(&mut self, stop_session: bool) -> Result<()> {
        info!("Stop backend");
        self.stop_backend().await?;

        if stop_session {
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
        }

        self.stop_hostevents().await?;

        Ok(())
    }

    async fn start_streaming_session(
        &mut self,
        streaming_config: StreamingSessionConfig,
    ) -> Result<()> {
        let msg_sender = self.message_sender.clone();

        info!("Start streaming session");
        streaming_config.print();

        match streaming_config.protocol.clone() {
            #[cfg(feature = "websocket")]
            StreamingProtocol::Websocket => {
                let config = ws_backend::WsBackendConfig {
                    http_client: &self.http_client,
                    streaming_config,
                    msg_sender,
                };

                let backend =
                    Box::new(ws_backend::WsBackend::new(config, self.metrics.is_some()).await?);
                self.backend = Some(backend);
            }
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(kymux_config) => {
                let config = kymux_backend::KymuxBackendConfig {
                    conn_params: &self.conn_params,
                    http_client: &self.http_client,
                    streaming_config,
                    kymux_config: kymux_config.clone(),
                    msg_sender,
                };

                self.backend = Some(kymux_backend::new(config, self.metrics.clone()).await?);
            }
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => {
                let config = rtp_backend::RtpBackendConfig {
                    conn_params: &self.conn_params,
                    http_client: &self.http_client,
                    streaming_config,
                    msg_sender,
                };

                let backend =
                    Box::new(rtp_backend::RtpBackend::new(config, self.metrics.is_some()).await?);
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
                    Err(err) => {
                        warn!("Failed to connect to ControlPlane: {err:?}");

                        Event::ControlPlaneConnectFailed
                    }
                };

                self.event_manager.send_event(event);

                Ok(())
            }
            Message::Disconnect { stop_session } => {
                info!("Disconnect requested by user");
                let ret = self.disconnect(*stop_session).await;

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

                let ret = self.stop_backend().await;

                self.event_manager.send_event(Event::DataPlaneClosed);

                ret
            }
            Message::StartStreamingSession { config } => {
                let ret = self.start_streaming_session(config.clone()).await;
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
                    let ret = self.disconnect(true).await;
                    if let Err(err) = ret {
                        warn!("Fail to disconnect client: {err:?}");
                    }
                }

                self.event_manager.send_event(event);

                self.bitrate_changed(config.bitrate)?;

                Ok(())
            }
            Message::StopStreamingSession => {
                self.stop_streaming_session().await?;

                self.event_manager.send_event(Event::StreamingStopped);

                Ok(())
            }
            Message::AudioSetVolume { volume } => {
                if let Some(backend) = &self.backend {
                    backend.audio_set_volume(*volume)?;
                }

                Ok(())
            }
            Message::AudioSetMute { mute } => {
                if let Some(backend) = &self.backend {
                    backend.audio_set_mute(*mute)?;
                }

                Ok(())
            }
            Message::AudioVolumeChanged { volume } => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.volume = Some(*volume);
                }

                self.event_manager
                    .send_event(Event::AudioVolumeChanged { volume: *volume });

                Ok(())
            }
            Message::AudioMuteChanged { mute } => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.mute = Some(*mute);
                }

                self.event_manager
                    .send_event(Event::AudioMuteChanged { mute: *mute });

                Ok(())
            }
            Message::SetBitrate { bitrate } => {
                host_service::set_bitrate(&self.http_client, *bitrate).await
            }
            Message::BitrateChanged { bitrate } => self.bitrate_changed(*bitrate),
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
                let ret = self.disconnect(true).await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::DecodeError,
                });

                ret
            }
            Message::AudioDecodeError => Ok(()),
            #[cfg(feature = "web")]
            Message::ImageDecoded { pts } => {
                self.event_manager
                    .send_event(Event::ImageDecoded { pts: *pts });

                Ok(())
            }
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
            Message::Metric(metric) => {
                let metrics = self
                    .metrics
                    .as_ref()
                    .expect("Received a metric while metrics are disabled");
                if let Err(err) = metrics.send(metric.clone()) {
                    warn!("Metric discarded: {err}");
                }
                Ok(())
            }
        }
    }

    fn bitrate_changed(&self, bitrate: u32) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap();
            state.bitrate = Some(bitrate);
        }

        self.event_manager
            .send_event(Event::BitrateChanged { bitrate });

        Ok(())
    }
}

#[cfg(feature = "kymux")]
pub use kymux_backend::{KymuxConfig, KymuxTransportBackend};

#[cfg(feature = "kymux")]
pub use kymux::kyproto::{AudioProtocol, VideoProtocol};

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

#[derive(Clone, Debug)]
pub struct AudioSessionConfig {
    pub audio_buffer: Option<u32>, // None means default value
}

impl AudioSessionConfig {
    pub(crate) const DEFAULT_AUDIO_BUFFER: u32 = 50; // milliseconds

    pub(crate) fn audio_buffer(&self) -> u32 {
        self.audio_buffer
            .unwrap_or(AudioSessionConfig::DEFAULT_AUDIO_BUFFER)
    }
}

#[derive(Clone)]
pub struct InputSessionConfig {
    pub pipeline: InputPipeline,
    pub gamepad_force_xbox360: Option<bool>,
    pub clipboard: bool,
}

#[derive(Clone)]
pub struct StreamingSessionConfig {
    pub protocol: StreamingProtocol,
    pub bitrate: u32,
    pub video: Vec<VideoSessionConfig>,
    pub audio: Option<AudioSessionConfig>,
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
                info!("\t\tvideo_buffer: {:}", config.player.video_buffer);
            }
        } else {
            info!("\tvideo: false");
        };

        if let Some(audio) = &self.audio {
            info!("\taudio: true");
            info!("\t\taudio_buffer: {}", audio.audio_buffer());
        } else {
            info!("\taudio: false");
        }

        if let Some(input) = &self.input {
            info!("\tinput: true");
            info!(
                "\t\tgamepad_force_xbox360: {:?}",
                input.gamepad_force_xbox360
            );
            info!("\t\tclipboard: {:?}", input.clipboard);
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
    msg_receiver: MessageReceiver,
    handler: Handler,
    metrics_collector: Option<MetricsCollector>,
    metrics_cb: Arc<Mutex<Option<Box<MetricsCallback>>>>,
}

impl MainLoop {
    fn new(
        msg_receiver: MessageReceiver,
        handler: Handler,
        metrics_collector: Option<MetricsCollector>,
        metrics_cb: Arc<Mutex<Option<Box<MetricsCallback>>>>,
    ) -> Self {
        Self {
            msg_receiver,
            handler,
            metrics_collector,
            metrics_cb,
        }
    }

    async fn process_metrics(
        metrics_collector: &mut MetricsCollector,
        metrics_cb: Arc<Mutex<Option<Box<MetricsCallback>>>>,
    ) {
        while let Some(metric) = metrics_collector.recv().await {
            if let Some(cb) = &*metrics_cb.lock().unwrap() {
                cb(metric.clone());
            }
        }
    }

    async fn run(mut self) {
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();

        if let Some(mut metrics_collector) = self.metrics_collector.take() {
            let metrics_cb = self.metrics_cb.clone();
            runtime::spawn(async move {
                tokio::select! {
                    _ = stop_rx => {}
                    _ = Self::process_metrics(&mut metrics_collector, metrics_cb) => {}
                }
            });
        }

        loop {
            let wrapper = self.msg_receiver.recv().await;
            if let Some(wrapper) = wrapper {
                let msg = wrapper.msg();
                let ret = self.handler.handle_message(msg).await;
                if let Err(err) = ret {
                    warn!(
                        "Processing of message {:?} failed with error {:?}",
                        msg, err
                    );
                }

                wrapper.complete();
            } else {
                break;
            }
        }

        let _ = stop_tx.send(()); // ignore send errors
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub volume: Option<f32>,
    pub mute: Option<bool>,
    pub bitrate: Option<u32>,
}

pub type MetricsCallback = dyn Fn(Metric) + Send + Sync;

pub struct Client {
    event_manager: EventManager,
    msg_sender: MessageSender,

    mainloop_engine: MainLoopEngine,
    state: Arc<Mutex<State>>,

    metrics_cb: Arc<Mutex<Option<Box<MetricsCallback>>>>,
}

impl Client {
    pub fn new(connect_config: ConnectConfig, metrics: bool) -> Result<Self> {
        let event_manager = EventManager::new();

        let (msg_sender, msg_receiver) = message_bus::create();
        let event_manager_handler = event_manager.clone();
        let msg_sender_handler = msg_sender.clone();

        let metrics_collector = if metrics {
            Some(MetricsCollector::new())
        } else {
            None
        };
        let metrics_producer = metrics_collector.as_ref().map(|mc| mc.producer());

        let state = Arc::new(Mutex::new(State::default()));

        let handler = Handler::new(
            connect_config,
            event_manager_handler,
            msg_sender_handler,
            metrics_producer,
            state.clone(),
        )?;

        let metrics_cb = Arc::new(Mutex::new(None));
        let mainloop = MainLoop::new(msg_receiver, handler, metrics_collector, metrics_cb.clone());

        let mainloop_engine = MainLoopEngine::new(mainloop);

        Ok(Self {
            event_manager,
            msg_sender,
            mainloop_engine,
            state,
            metrics_cb,
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

    #[cfg(target_arch = "wasm32")]
    pub async fn disconnect_and_stop(self, stop_session: bool) -> Result<()> {
        self.msg_sender
            .send_async_and_wait_completion(Message::Disconnect { stop_session })
            .await?;

        self.mainloop_engine.stop().await;

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn disconnect_and_stop(&self, stop_session: bool) -> Result<()> {
        self.msg_sender
            .send_and_wait_completion(Message::Disconnect { stop_session })?;

        self.mainloop_engine.stop()
    }

    pub fn connect(&self) -> Result<()> {
        self.msg_sender.send(Message::Connect)?;
        Ok(())
    }

    pub fn disconnect(&self) -> Result<()> {
        self.msg_sender
            .send(Message::Disconnect { stop_session: true })?;
        Ok(())
    }

    pub fn start_streaming(&self, config: StreamingSessionConfig) -> Result<()> {
        self.msg_sender
            .send(Message::StartStreamingSession { config })?;
        Ok(())
    }

    pub fn stop_streaming(&self) -> Result<()> {
        self.msg_sender.send(Message::StopStreamingSession)?;
        Ok(())
    }

    pub fn audio_set_volume(&self, volume: f32) -> Result<()> {
        self.msg_sender.send(Message::AudioSetVolume { volume })?;
        Ok(())
    }

    pub fn audio_set_mute(&self, mute: bool) -> Result<()> {
        self.msg_sender.send(Message::AudioSetMute { mute })?;
        Ok(())
    }

    pub fn set_bitrate(&self, bitrate: u32) -> Result<()> {
        self.msg_sender.send(Message::SetBitrate { bitrate })?;
        Ok(())
    }

    pub fn get_event_manager(&self) -> EventManager {
        self.event_manager.clone()
    }

    pub fn get_state(&self) -> State {
        let state = self.state.lock().unwrap();
        (*state).clone()
    }

    #[cfg(feature = "js-player")]
    pub async fn supported_video_codecs(
        force_software: bool,
    ) -> Vec<platform::player::VideoDecoderConfig> {
        platform::player::supported_video_codecs(force_software).await
    }

    pub fn set_metrics_callback(&self, cb: Option<Box<MetricsCallback>>) {
        *self.metrics_cb.lock().unwrap() = cb;
    }
}
