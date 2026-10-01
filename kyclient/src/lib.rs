// Project Kyber: lib.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

mod enums;
mod error;
pub mod event_manager;
mod host_events;
mod host_service;
pub(crate) mod http;
mod message_bus;
pub mod metrics;
pub(crate) mod runtime;
mod util;

#[cfg_attr(not(target_family = "wasm"), path = "./platform/desktop/mod.rs")]
#[cfg_attr(target_family = "wasm", path = "./platform/web/mod.rs")]
mod platform;

#[cfg(feature = "capi")]
pub mod capi;

#[cfg(feature = "kymux")]
mod kymux_backend;

#[cfg(feature = "rtp")]
mod rtp_backend;

#[cfg(not(target_family = "wasm"))]
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use log::{debug, info, warn};
use serde::Serialize;

pub use crate::error::{Error, Result};
use crate::util::*;
use enums::Message;
pub use enums::{
    ConnectFailedReason, ControlPlaneCloseReason, DataPlaneCloseReason, Display, Event, StopReason,
    TlsRootCert,
};
use event_manager::EventManager;
pub use event_manager::EventSink;
use host_events::HostEventReceiver;
pub use host_events::{StreamerPipelineEventType, StreamerPipelineStage, StreamerPipelineType};
use http::HttpClient;
pub use kyclient_common::{auth::AuthCredentials, metrics::Metric};
pub(crate) use message_bus::{MessageReceiver, MessageSender};
use metrics::{MetricsCollector, MetricsProducer};
pub use platform::{ConnectConfig, VideoPlayerConfig};
pub(crate) use platform::{ConnectionParams, InputPipeline, MainLoopEngine};
#[cfg(not(target_family = "wasm"))]
pub use platform::{KnownHosts, VerifyMode, get_resource_path};

#[cfg(target_family = "wasm")]
pub use platform::player;

pub fn init_crypto() {
    #[cfg(not(target_family = "wasm"))]
    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap();
}

#[allow(dead_code)]
pub(crate) const STREAMER_PORT: u16 = 8080;

pub(crate) use kyctl_common::capabilities::{Capabilities, ControlPlaneCapabilities};

/// Peer information resolved from the HTTP `/capabilities` response.
pub(crate) struct PeerInfo {
    /// Peer address resolved by the HTTP stack (Happy Eyeballs / RFC 8305).
    /// Reused for QUIC so it gets the same IPv4/IPv6 choice.
    /// Kept as a `SocketAddr` to preserve the IPv6 scope id of link-local peers.
    #[cfg(not(target_family = "wasm"))]
    pub(crate) addr: SocketAddr,
}

#[cfg(not(target_family = "wasm"))]
impl PeerInfo {
    pub(crate) fn addr_with_port(&self, port: u16) -> SocketAddr {
        let mut addr = self.addr;
        addr.set_port(port);
        addr
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
trait BackendDriver: KySend {
    fn audio_set_volume(&self, volume: f32) -> Result<()>;

    fn audio_set_mute(&self, mute: bool) -> Result<()>;

    #[cfg(target_family = "wasm")]
    fn audio_try_resume(&self) -> player::AudioResumeState;

    #[cfg(target_family = "wasm")]
    async fn audio_resume(&self) -> Result<()>;

    fn input_service_connected(&mut self) -> Result<()>;

    fn input_service_disconnected(&mut self) -> Result<()>;

    async fn stop(&mut self) -> Result<()>;
}

/// Manages automatic reconnection behavior when connection is lost.
///
/// Reconnection is automatically triggered on connection loss (not on user disconnect),
/// but only if `enabled` is set to true via `ConnectConfig::auto_reconnection`.
/// The strategy is to retry multiple times with a fixed interval between attempts.
///
/// # State Model
/// - `enabled`: Whether auto-reconnection is enabled (from ConnectConfig)
/// - `control_should_be_up`: Intent - whether control plane should be connected
/// - `streaming_should_be_up`: Intent - whether streaming should be active
/// - `reconnecting`: Whether a reconnection attempt is in progress
///
/// # Events
/// - `Reconnecting`: Emitted for each reconnection attempt
/// - `Reconnected`: Emitted when reconnection succeeds with `should_restart_streaming` flag
/// - `ReconnectionFailed`: Emitted when all attempts are exhausted
///
/// # Default Policy
/// - Auto-reconnection disabled by default
/// - 10 reconnection attempts (when enabled)
/// - 1 second interval between attempts
///
/// # Application Responsibilities
/// The application is responsible for:
/// - Enabling auto-reconnection via `ConnectConfig::auto_reconnection = true`
/// - Handling reconnection events (`Reconnecting`, `Reconnected`, `ReconnectionFailed`)
/// - Saving the streaming configuration when starting streaming
/// - Restarting streaming when receiving `Reconnected` with `should_restart_streaming=true`
struct ReconnectionState {
    enabled: bool,
    should_be_up: Option<ControlUpIntent>,
    max_attempts: u32,
    interval: Duration,
}

impl ReconnectionState {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            should_be_up: None,
            max_attempts: 10,
            interval: Duration::from_secs(1),
        }
    }
}

#[derive(Default)]
struct ControlUpIntent {
    current_attempt: u32,
    reconnection_task: Option<runtime::Task>,
    data_up: bool,
}

enum ConnectFullOutcome {
    Connected {
        connection: Connection,
        displays: Vec<Display>,
    },
    AuthenticationRequired {
        issuer: String,
        client_id: String,
    },
}

enum ConnectOutcome {
    Connected { displays: Vec<Display> },
    AuthenticationRequired,
}

struct Handler {
    conn_params: ConnectionParams,
    http_client: HttpClient,
    event_manager: EventManager,
    message_sender: MessageSender,
    metrics: Option<MetricsProducer>,

    state: Arc<Mutex<State>>,
    reconnection_state: ReconnectionState,

    connection: Option<Connection>,
}

struct Connection {
    #[allow(dead_code)] // This field might not be used depending on enabled features
    peer_info: PeerInfo,
    host_events: HostEventReceiver,

    backend: Option<Box<dyn BackendDriver>>,
}

impl Connection {
    async fn stop(self) {
        if let Some(mut backend) = self.backend {
            info!("Stop backend");
            if let Err(err) = backend.stop().await {
                warn!("Failed to stop backend: {err:?}");
            }
        }

        drop(self.host_events);
        info!("HostEventHandler stopped");
    }
}

impl Handler {
    fn new(
        connect_config: ConnectConfig,
        event_manager: EventManager,
        message_sender: MessageSender,
        metrics: Option<MetricsProducer>,
        state: Arc<Mutex<State>>,
    ) -> Result<Self> {
        let auto_reconnection = connect_config.auto_reconnection;
        let conn_params = ConnectionParams::new(connect_config)?;
        let http_client = HttpClient::new(conn_params.clone())?;

        Ok(Self {
            conn_params,
            http_client,
            event_manager,
            message_sender,
            metrics,
            state,
            reconnection_state: ReconnectionState::new(auto_reconnection),

            connection: None,
        })
    }

    // This method uses a unique reference even though it does not mutable `self`
    // to avoid making the resulting future `!Send`.
    async fn connect(&mut self) -> Result<ConnectFullOutcome> {
        // Resolve the peer address from the HTTP response so we can later
        // connect QUIC to the same IP.  This matters because QUIC stack on
        // desktop does not implement Happy Eyeballs (RFC 8305) so we let
        // the HTTP stack pick the right address (IPv4 vs IPv6) and reuse that
        // choice for QUIC.
        //
        // On browsers this address is unavailable, but the WebTransport API
        // performs its own Happy Eyeballs resolution internally.
        info!("Get Host capabilities");
        let (capabilities, _peer_addr) = host_service::capabilities(&self.http_client).await?;

        let peer_info = PeerInfo {
            // On desktop, peer address is required for QUIC and WebSocket connections.
            // Fail early if the HTTP stack didn't expose it.
            #[cfg(not(target_family = "wasm"))]
            addr: _peer_addr.ok_or(Error::NoPeerAddress)?,
        };

        let response =
            match host_service::login(&self.http_client, &self.conn_params.credentials).await? {
                host_service::LoginResult::OidcRequired(challenge) => {
                    let host_service::OidcChallenge { issuer, client_id } = challenge;
                    return Ok(ConnectFullOutcome::AuthenticationRequired { issuer, client_id });
                }
                host_service::LoginResult::Success(response) => response,
            };

        info!("Logged in with uuid '{}'", response.uid);

        info!("Connect to Control Plane");
        let host_events = HostEventReceiver::new(
            &self.conn_params,
            &peer_info,
            &capabilities.control_plane,
            self.message_sender.clone(),
            &response.websocket,
        )
        .await?;

        // Enumerate displays
        info!("Enumerate displays");
        let displays = host_service::enumerate_displays(&self.http_client).await?;
        info!("Got displays: {displays:?}");

        let connection = Connection {
            peer_info,
            host_events,

            backend: None,
        };

        Ok(ConnectFullOutcome::Connected {
            connection,
            displays,
        })
    }

    async fn connect_and_handle(&mut self) -> Result<ConnectOutcome> {
        Ok(match self.connect().await? {
            ConnectFullOutcome::Connected {
                connection,
                displays,
            } => {
                self.connection = Some(connection);
                ConnectOutcome::Connected { displays }
            }
            ConnectFullOutcome::AuthenticationRequired { issuer, client_id } => {
                self.event_manager
                    .send_event(Event::AuthenticationRequired { issuer, client_id });
                ConnectOutcome::AuthenticationRequired
            }
        })
    }

    async fn stop_backend(&mut self) {
        info!("Stop backend");
        if let Some(conn) = &mut self.connection
            && let Some(mut backend) = conn.backend.take()
            && let Err(err) = backend.stop().await
        {
            warn!("Failed to stop backend: {err:?}");
        }
    }

    async fn disconnect(&mut self, stop_session: bool) {
        if stop_session {
            info!("Request to log out and stop host session");
            let ret = host_service::logout(&self.http_client).await;
            if let Err(err) = ret {
                warn!(
                    "Logout request rejected by controller ({:?}). Stop client anyway.",
                    err
                );
            } else {
                info!("Logged out, session stopped");
            }
        }

        if let Some(conn) = self.connection.take() {
            conn.stop().await;
        }
    }

    async fn start_streaming_session(
        &mut self,
        streaming_config: StreamingSessionConfig,
    ) -> Result<()> {
        let Some(conn) = &mut self.connection else {
            return Ok(());
        };

        let msg_sender = self.message_sender.clone();

        info!("Start streaming session");
        streaming_config.print();

        match streaming_config.protocol.clone() {
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(kymux_config) => {
                let config = kymux_backend::KymuxBackendConfig {
                    conn_params: &self.conn_params,
                    peer_info: &conn.peer_info,
                    http_client: &self.http_client,
                    streaming_config,
                    kymux_config,
                    msg_sender,
                };

                let backend = Box::new(
                    kymux_backend::KymuxBackend::new(config, self.metrics.as_ref()).await?,
                );
                conn.backend = Some(backend);
            }
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => {
                let config = rtp_backend::RtpBackendConfig {
                    peer_info: &conn.peer_info,
                    http_client: &self.http_client,
                    streaming_config,
                    msg_sender,
                };

                let backend =
                    Box::new(rtp_backend::RtpBackend::new(config, self.metrics.is_some()).await?);
                conn.backend = Some(backend);
            }
        };

        Ok(())
    }

    async fn stop_streaming_session(&mut self) {
        info!("Request to stop streaming");
        match host_service::stop_streaming(&self.http_client).await {
            Ok(()) => info!("Streaming stopped"),
            Err(err) => {
                warn!("Stop streaming rejected by controller ({err:?}). Stop client anyway.")
            }
        }

        self.stop_backend().await;
    }

    fn schedule_reconnection(&mut self) -> bool {
        if !self.reconnection_state.enabled {
            return false;
        }

        let Some(intent) = &mut self.reconnection_state.should_be_up else {
            return false;
        };

        if intent.reconnection_task.is_some() {
            return false;
        }

        if intent.current_attempt >= self.reconnection_state.max_attempts {
            return false;
        }

        intent.current_attempt += 1;
        let attempt = intent.current_attempt;
        let max_attempts = self.reconnection_state.max_attempts;

        self.event_manager.send_event(Event::Reconnecting {
            attempt,
            max_attempts,
        });

        // Schedule reconnect message
        let msg_sender = self.message_sender.clone();
        let interval = self.reconnection_state.interval;
        let task = runtime::Task::spawn(async move {
            runtime::sleep(interval).await;
            let _ = msg_sender.send(Message::Reconnect { attempt });
        });

        // Store task handle so it can be cancelled if needed
        intent.reconnection_task = Some(task);

        true
    }

    fn on_reconnection_failure(&mut self, err: Error, attempt: u32) {
        warn!("Reconnection attempt {attempt} failed: {err:?}");

        if self.schedule_reconnection() {
            return;
        }

        // All attempts exhausted
        warn!("Reconnection failed after {attempt} attempts");
        self.event_manager.send_event(Event::ReconnectionFailed);
        self.event_manager.send_event(Event::Stopped {
            reason: StopReason::ControlPlaneClosed,
        });
    }

    async fn handle_message(&mut self, msg: Message) -> Result<()> {
        match msg {
            Message::Connect => {
                // Set intent: control plane should be up
                self.reconnection_state.should_be_up = Some(ControlUpIntent::default());

                match self.connect_and_handle().await {
                    Ok(ConnectOutcome::Connected { displays }) => {
                        self.event_manager
                            .send_event(Event::ControlPlaneConnected { displays });
                    }
                    Ok(ConnectOutcome::AuthenticationRequired) => (),
                    Err(err) => {
                        warn!("Failed to connect to ControlPlane: {err:?}");

                        let reason = err.into();
                        self.event_manager
                            .send_event(Event::ControlPlaneConnectFailed { reason });
                    }
                }
            }
            Message::Disconnect { stop_session } => {
                info!("Disconnect requested by user");

                // Clear intent flags and cancel any pending reconnection
                self.reconnection_state.should_be_up = None;

                self.disconnect(stop_session).await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::Requested,
                });
            }
            Message::Reconnect { attempt } => {
                let Some(intent) = &mut self.reconnection_state.should_be_up else {
                    return Ok(());
                };

                intent.reconnection_task = None;
                let should_restart_streaming = intent.data_up;

                info!("Attempting reconnection (attempt {attempt})");

                match self.connect_and_handle().await {
                    Ok(res) => {
                        if let Some(intent) = &mut self.reconnection_state.should_be_up {
                            intent.current_attempt = 0;
                        }

                        if let ConnectOutcome::Connected { .. } = res {
                            info!("Control plane reconnected successfully");

                            self.event_manager.send_event(Event::Reconnected {
                                should_restart_streaming,
                            });
                        }
                    }
                    Err(err) => self.on_reconnection_failure(err, attempt),
                }
            }
            Message::ControlPlaneClosed { reason } => {
                warn!("ControlPlaneClosed. Reason: {reason:?}");

                if let Some(conn) = self.connection.take() {
                    conn.stop().await;
                }

                // Schedule reconnection if enabled
                if self.schedule_reconnection() {
                    return Ok(());
                }

                // If reconnection disabled or exhausted, stop
                let reason = match reason {
                    ControlPlaneCloseReason::Eof => StopReason::ControlPlaneClosed,
                    ControlPlaneCloseReason::Closed { .. } => StopReason::ControlPlaneClosed,
                    ControlPlaneCloseReason::Timeout => StopReason::ControlPlaneTimeout,
                };

                self.event_manager.send_event(Event::Stopped { reason });
            }
            Message::DataPlaneClosed { reason } => {
                warn!("DataPlaneClosed. Reason: {reason:?}");

                self.stop_backend().await;

                self.event_manager.send_event(Event::DataPlaneClosed);

                // Schedule reconnection if enabled
                self.schedule_reconnection();
            }
            Message::StartStreamingSession { config } => {
                match &mut self.reconnection_state.should_be_up {
                    Some(intent) => intent.data_up = true,
                    _ => return Ok(()),
                }

                let bitrate = config.bitrate;
                let ret = self.start_streaming_session(config).await;
                let event = match &ret {
                    Ok(()) => Event::StreamingStarted,
                    Err(Error::DataPlaneConnectFailed) => {
                        warn!("Dataplane connect failed. Stop client");
                        Event::DataPlaneConnectFailed
                    }
                    Err(err) => {
                        warn!("Streaming start failed: {err:?}. Stop client");
                        Event::StreamingStartFailed
                    }
                };

                if ret.is_err() {
                    self.disconnect(true).await;
                }

                self.event_manager.send_event(event);

                self.bitrate_changed(bitrate)?;
            }
            Message::StopStreamingSession => {
                if let Some(intent) = &mut self.reconnection_state.should_be_up {
                    intent.data_up = false;
                }

                self.stop_streaming_session().await;

                self.event_manager.send_event(Event::StreamingStopped);
            }
            Message::AudioSetVolume { volume } => {
                if let Some(conn) = &self.connection
                    && let Some(backend) = &conn.backend
                {
                    backend.audio_set_volume(volume)?;
                }
            }
            Message::AudioSetMute { mute } => {
                if let Some(conn) = &self.connection
                    && let Some(backend) = &conn.backend
                {
                    backend.audio_set_mute(mute)?;
                }
            }
            #[cfg(target_family = "wasm")]
            Message::AudioTryResume { respond } => {
                let mut state = player::AudioResumeState::Absent;

                if let Some(conn) = &self.connection
                    && let Some(backend) = &conn.backend
                {
                    state = backend.audio_try_resume();
                }

                *respond.borrow_mut() = Some(state);
            }
            #[cfg(target_family = "wasm")]
            Message::AudioResume { respond } => {
                let mut result = Ok(());

                if let Some(conn) = &self.connection
                    && let Some(backend) = &conn.backend
                {
                    result = backend.audio_resume().await;
                }

                *respond.borrow_mut() = Some(result);
            }
            Message::AudioVolumeChanged { volume } => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.volume = Some(volume);
                }

                self.event_manager
                    .send_event(Event::AudioVolumeChanged { volume });
            }
            Message::AudioMuteChanged { mute } => {
                {
                    let mut state = self.state.lock().unwrap();
                    state.mute = Some(mute);
                }

                self.event_manager
                    .send_event(Event::AudioMuteChanged { mute });
            }
            #[cfg(target_family = "wasm")]
            Message::AudioStateChanged { state } => {
                self.event_manager
                    .send_event(Event::AudioStateChanged { state });
            }
            Message::SetBitrate { bitrate } => {
                host_service::set_bitrate(&self.http_client, bitrate).await?
            }
            Message::BitrateChanged { bitrate } => self.bitrate_changed(bitrate)?,
            Message::PlayerStarted => {
                info!("Player started");
            }
            Message::PlayerStopped => {
                info!("Player stopped");
            }
            Message::InputServiceConnected => {
                info!("Input service connected");

                if let Some(conn) = &mut self.connection
                    && let Some(backend) = &mut conn.backend
                {
                    backend.input_service_connected()?;
                }

                self.event_manager.send_event(Event::InputServiceConnected);
            }
            Message::InputServiceDisconnected => {
                info!("Input service disconnected");

                if let Some(conn) = &mut self.connection
                    && let Some(backend) = &mut conn.backend
                {
                    backend.input_service_disconnected()?;
                }

                self.event_manager
                    .send_event(Event::InputServiceDisconnected);
            }
            Message::DisplayListUpdated { displays } => {
                info!("Display list updated: {displays:?}");

                self.event_manager
                    .send_event(Event::DisplayListUpdated { displays });
            }
            Message::StreamerPipelineEvent(event) => {
                self.event_manager.send_event(Event::StreamerPipelineEvent {
                    type_: event.type_,
                    stage: event.stage,
                    event: event.event,
                });
            }
            Message::VideoDecodeError => {
                self.disconnect(true).await;

                self.event_manager.send_event(Event::Stopped {
                    reason: StopReason::DecodeError,
                });
            }
            Message::AudioDecodeError => (),
            #[cfg(target_family = "wasm")]
            Message::ImageDecoded { pts } => {
                self.event_manager.send_event(Event::ImageDecoded { pts });
            }
            #[cfg(target_family = "wasm")]
            Message::CanvasResized {
                width,
                height,
                frame_coded_width,
                frame_coded_height,
            } => {
                self.event_manager.send_event(Event::CanvasResized {
                    width,
                    height,
                    frame_coded_width,
                    frame_coded_height,
                });
            }
            Message::UpdateCredentials(credentials) => {
                info!("Credentials updated");
                self.conn_params.credentials = credentials;
            }
            Message::RenewSession(credentials) => {
                self.conn_params.credentials = credentials;
                if self.connection.is_none() {
                    self.message_sender.send(Message::Connect)?;
                    return Ok(());
                }
                match host_service::renew_session(&self.http_client, &self.conn_params.credentials)
                    .await
                {
                    Ok(host_service::RenewResult::Renewed) => {
                        info!("Session renewed");
                    }
                    Ok(host_service::RenewResult::SessionGone) => {
                        info!("Session gone on renewal, reconnecting");
                        self.message_sender.send(Message::Connect)?;
                    }
                    Err(e) => {
                        warn!("Session renewal failed: {e:?}, reconnecting");
                        self.message_sender.send(Message::Connect)?;
                    }
                }
            }
        }

        Ok(())
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
pub use kyctl_common::kymux::{AudioProtocol, KyProtoConfig, KymuxConfig, VideoProtocol};

#[derive(Clone, Debug)]
pub enum StreamingProtocol {
    #[cfg(feature = "rtp")]
    Rtp,
    #[cfg(feature = "kymux")]
    Kymux(KymuxConfig),
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[repr(C)]
pub enum VideoCodec {
    H264,
    HEVC,
    AV1,
}

impl From<VideoCodec> for kyctl_common::enums::VideoCodec {
    fn from(c: VideoCodec) -> Self {
        match c {
            VideoCodec::H264 => Self::H264,
            VideoCodec::HEVC => Self::HEVC,
            VideoCodec::AV1 => Self::AV1,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub enum CursorFormat {
    Raw,
    Png,
}

impl From<CursorFormat> for kyctl_common::enums::CursorFormat {
    fn from(c: CursorFormat) -> Self {
        match c {
            CursorFormat::Raw => Self::Raw,
            CursorFormat::Png => Self::Png,
        }
    }
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

        match self.protocol {
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => {
                info!("\tprotocol: RTP");
            }
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(ref config) => {
                info!("\tprotocol: Kymux");
                match config {
                    KymuxConfig::KyProto(config) => {
                        info!("\t\tvideo_protocol: {:?}", config.video_protocol);
                        info!("\t\taudio_protocol: {:?}", config.audio_protocol);
                    }
                    KymuxConfig::KyWebSocket => {
                        info!("\t\tbackend: kywebsocket");
                    }
                }
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
    metrics_cb: KyArc<KyMutex<Option<Box<MetricsCallback>>>>,
}

impl MainLoop {
    fn new(
        msg_receiver: MessageReceiver,
        handler: Handler,
        metrics_collector: Option<MetricsCollector>,
        metrics_cb: KyArc<KyMutex<Option<Box<MetricsCallback>>>>,
    ) -> Self {
        Self {
            msg_receiver,
            handler,
            metrics_collector,
            metrics_cb,
        }
    }

    async fn run(mut self) {
        let _metrics_task;

        if let Some(mut metrics_collector) = self.metrics_collector.take() {
            let metrics_cb = self.metrics_cb.clone();

            _metrics_task = runtime::Task::spawn(async move {
                while let Some(metric) = metrics_collector.recv().await {
                    if let Some(cb) = &mut *metrics_cb.lock() {
                        cb(metric.clone());
                    }
                }
            });
        }

        while let Some(wrapper) = self.msg_receiver.recv().await {
            debug!("Got message: {:?}", wrapper.msg);

            let ret = self.handler.handle_message(wrapper.msg).await;
            if let Err(err) = ret {
                warn!("Processing of message failed with error {err:?}");
            }

            if let Some(completion_cb) = wrapper.completion_cb {
                completion_cb();
            };
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub volume: Option<f32>,
    pub mute: Option<bool>,
    pub bitrate: Option<u32>,
}

#[cfg(not(target_family = "wasm"))]
pub type MetricsCallback = dyn FnMut(Metric) + Send + Sync;
#[cfg(target_family = "wasm")]
pub type MetricsCallback = dyn FnMut(Metric);

pub struct Client {
    event_manager: EventManager,
    msg_sender: MessageSender,

    mainloop_engine: MainLoopEngine,
    state: Arc<Mutex<State>>,

    metrics_cb: KyArc<KyMutex<Option<Box<MetricsCallback>>>>,
}

impl Client {
    pub fn new(connect_config: ConnectConfig, metrics: bool) -> Result<Self> {
        let event_manager = EventManager::new();

        let (msg_sender, msg_receiver) = message_bus::create();
        let metrics_collector = if metrics {
            Some(MetricsCollector::new())
        } else {
            None
        };
        let metrics_producer = metrics_collector.as_ref().map(|mc| mc.producer());

        let state = Arc::new(Mutex::new(State::default()));

        let handler = Handler::new(
            connect_config,
            event_manager.clone(),
            msg_sender.clone(),
            metrics_producer,
            state.clone(),
        )?;

        let metrics_cb = KyArc::new(KyMutex::new(None));
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

    // Kept async for backward compatibility for now
    #[allow(clippy::unused_async)]
    #[cfg(target_arch = "wasm32")]
    pub async fn stop(self) -> Result<()> {
        self.mainloop_engine.stop();
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn stop(&self) -> Result<()> {
        self.mainloop_engine.stop();
        Ok(())
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn disconnect_and_stop(self, stop_session: bool) -> Result<()> {
        self.msg_sender
            .send_async_and_wait_completion(Message::Disconnect { stop_session })
            .await?;

        self.mainloop_engine.stop();

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn disconnect_and_stop(&self, stop_session: bool) -> Result<()> {
        self.msg_sender
            .send_and_wait_completion(Message::Disconnect { stop_session })?;

        self.mainloop_engine.stop();
        Ok(())
    }

    pub fn connect(&self) -> Result<()> {
        self.msg_sender.send(Message::Connect)?;
        Ok(())
    }

    pub fn update_credentials(&self, credentials: AuthCredentials) -> Result<()> {
        self.msg_sender
            .send(Message::UpdateCredentials(credentials))?;
        Ok(())
    }

    /// Queues a session renewal with updated credentials. Returns `Ok(())` once the message is
    /// enqueued, not when the renewal completes. If the server-side session is gone or the
    /// request fails, the client falls back to a full reconnect using the new credentials.
    /// Queues a session renewal with updated credentials. Returns `Ok(())` once the message is enqueued, not when the renewal completes. If the server-side session is gone or the request fails, the client falls back to a full reconnect using the new credentials.
    pub fn renew_session(&self, credentials: AuthCredentials) -> Result<()> {
        self.msg_sender.send(Message::RenewSession(credentials))?;
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

    /// Probes the AudioContext state and fires a non-blocking resume attempt. Returns `Absent`
    /// until the backend has created the worklet, so callers can poll until it reports `Running`
    /// or `Suspended`.
    #[cfg(target_family = "wasm")]
    pub async fn audio_try_resume(&self) -> player::AudioResumeState {
        let respond = std::rc::Rc::new(std::cell::RefCell::new(None));
        if self
            .msg_sender
            .send_async_and_wait_completion(Message::AudioTryResume {
                respond: respond.clone(),
            })
            .await
            .is_err()
        {
            return player::AudioResumeState::Absent;
        }

        let state = respond.borrow_mut().take();
        state.unwrap_or(player::AudioResumeState::Absent)
    }

    /// Resumes the AudioContext. Call this from a real user-gesture handler so the browser allows
    /// the context to leave the suspended state.
    #[cfg(target_family = "wasm")]
    pub async fn audio_resume(&self) -> Result<()> {
        let respond = std::rc::Rc::new(std::cell::RefCell::new(None));
        self.msg_sender
            .send_async_and_wait_completion(Message::AudioResume {
                respond: respond.clone(),
            })
            .await?;

        let result = respond.borrow_mut().take();
        result.unwrap_or(Ok(()))
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

    pub fn set_metrics_callback(&self, cb: Option<Box<MetricsCallback>>) {
        *self.metrics_cb.lock() = cb;
    }
}
