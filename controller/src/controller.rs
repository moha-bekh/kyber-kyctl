use std::collections::HashMap;
use std::sync::Arc;

use log::{info, warn};
use time::OffsetDateTime;
use uuid::Uuid;

use avservice_types::Display;
use libkypc::process::ProcessFactory;

use crate::auth;
use crate::avservice;
use crate::config;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::kymux_controller::{KymuxBackend, SharedKymuxInfra};
use crate::session::ClientAuthTokens;
use crate::tls;
use crate::ws::WebsocketBackend;
use crate::HostEventSender;
use crate::StateMachineProxy;

pub(crate) enum DataPlaneBackend {
    Rtp,
    Kymux(KymuxBackend),
    Websocket(WebsocketBackend),
}

pub(crate) struct StreamingSession {
    pub(crate) host_event_sender: HostEventSender,
    pub(crate) data_plane: Option<DataPlaneBackend>,
}

impl StreamingSession {
    pub(crate) fn new(host_event_sender: HostEventSender) -> Self {
        Self {
            host_event_sender,
            data_plane: None,
        }
    }

    pub(crate) async fn stop_dataplane(&mut self) -> Result<()> {
        match self.data_plane.take() {
            Some(DataPlaneBackend::Rtp) => {}
            Some(DataPlaneBackend::Kymux(backend)) => {
                backend.stop().await?;
            }
            Some(DataPlaneBackend::Websocket(backend)) => {
                backend.stop().await?;
            }
            None => {}
        }

        Ok(())
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        info!("Stop dataplane");
        self.stop_dataplane().await?;

        info!("Stop running HostEventSender");
        self.host_event_sender.stop().await?;

        Ok(())
    }
}

pub(crate) struct ClientSession {
    pub(crate) uid: Uuid,
    pub(crate) username: String,
    pub(crate) expiration: Option<OffsetDateTime>,
    pub(crate) auth_tokens: ClientAuthTokens,
    pub(crate) stream_session: Option<StreamingSession>,
    pub(crate) services: StreamingServices,
}

impl ClientSession {
    pub(crate) fn new(uid: Uuid, session: auth::Session) -> Self {
        Self {
            uid,
            username: session.username,
            expiration: session.expiration,
            auth_tokens: ClientAuthTokens::generate(),
            stream_session: None,
            services: Default::default(),
        }
    }
}

#[derive(Default)]
pub(crate) struct StreamingServices {
    pub(crate) video: Vec<avservice::Handler>,
    pub(crate) audio: Option<avservice::Handler>,
    pub(crate) input: Option<inputservice::Handler>,
    pub(crate) clock_sync: Option<crate::kymux_controller::ClockSyncTask>,
    pub(crate) metrics: Option<crate::kymux_controller::MetricsTask>,
}

impl StreamingServices {
    async fn stop_avservice(handler: Option<avservice::Handler>) -> Result<()> {
        let Some(mut handler) = handler else {
            return Ok(());
        };

        if !handler.is_running() {
            return Ok(());
        }

        info!("Stop running {:?} AVServer", handler.type_());
        handler.stop().await?;

        Ok(())
    }

    async fn stop_input(handler: Option<inputservice::Handler>) -> Result<()> {
        let Some(mut handler) = handler else {
            return Ok(());
        };

        if !handler.is_running() {
            return Ok(());
        }

        info!("Stop running InputServer");
        handler.stop().await?;

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        for service in self.video.drain(0..) {
            Self::stop_avservice(Some(service)).await?;
        }

        Self::stop_avservice(self.audio.take()).await?;
        Self::stop_input(self.input.take()).await?;

        if let Some(clock_sync) = self.clock_sync.take() {
            clock_sync.stop().await;
        }

        if let Some(metrics) = self.metrics.take() {
            metrics.stop().await;
        }

        Ok(())
    }
}

pub struct Controller {
    pub(crate) process_factory: Arc<ProcessFactory>,

    pub(crate) displays: Vec<Display>,

    pub(crate) config: config::Config,
    pub(crate) tls_config: tls::Config,

    pub(crate) sessions: HashMap<Uuid, ClientSession>,

    pub(crate) state_machine_proxy: Option<StateMachineProxy>,

    // Shared Kymux infrastructure (unified server supporting both QUIC and WebTransport on same port)
    pub(crate) shared_kymux_infra: Option<Arc<tokio::sync::Mutex<SharedKymuxInfra>>>,
}

impl Controller {
    pub(crate) fn new(
        process_factory: Arc<ProcessFactory>,
        config: config::Config,
        tls_config: tls::Config,
    ) -> Self {
        Self {
            process_factory,
            displays: vec![],
            config,
            tls_config,
            sessions: HashMap::new(),
            state_machine_proxy: None,
            shared_kymux_infra: None,
        }
    }

    pub fn get_session<'a>(&'a mut self, uid: &uuid::Uuid) -> Result<&'a mut ClientSession> {
        self.sessions
            .get_mut(uid)
            .ok_or(Error::InvalidSession { uid: *uid })
    }

    pub async fn stop_streaming(&mut self, session_id: Uuid) -> Result<()> {
        info!("Stop streaming for session {}", session_id);

        let client_session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(Error::InvalidSession { uid: session_id })?;

        if let Some(stream_session) = client_session.stream_session.as_mut() {
            let ret = stream_session.stop_dataplane().await;
            if let Err(err) = ret {
                warn!("Failed to stop client session: {err:?}");
            }
        }

        let ret = client_session.services.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop streaming services: {err:?}");
        }

        Ok(())
    }

    pub async fn stop(&mut self, session_id: Uuid) -> Result<()> {
        info!("Stop session {}", session_id);

        let Some(mut client_session) = self.sessions.remove(&session_id) else {
            return Err(Error::InvalidSession { uid: session_id });
        };

        if let Some(stream_session) = client_session.stream_session.take() {
            let ret = stream_session.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop client session: {err:?}");
            }
        }

        let ret = client_session.services.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop streaming services: {err:?}");
        }

        // Clean up shared Kymux infrastructure when no more sessions
        self.cleanup_shared_kymux_infra().await;

        Ok(())
    }

    pub(crate) async fn cleanup_shared_kymux_infra(&mut self) {
        if !self.sessions.is_empty() {
            return;
        }

        let Some(shared_infra) = self.shared_kymux_infra.take() else {
            return;
        };

        // Try to get exclusive access. This can fail if another async task still
        // holds a clone of the Arc. Known cases:
        // - start_kymux: clones Arc while registering session and creating backend
        // - start_session: clones Arc while waiting for connection and starting services
        // These can race with session termination (timeout, logout) causing try_unwrap
        // to fail. In that case, we restore the Arc and cleanup will be retried when
        // the next session ends or when the holding task completes.
        match Arc::try_unwrap(shared_infra) {
            Ok(infra_mutex) => {
                let infra = infra_mutex.into_inner();
                if let Err(err) = infra.stop().await {
                    warn!("Failed to stop shared Kymux infrastructure: {err:?}");
                }
            }
            Err(arc) => {
                info!("Deferring Kymux infrastructure cleanup: other references still held");
                self.shared_kymux_infra = Some(arc);
            }
        }
    }

    pub(crate) fn has_data_plane(&self, session_id: &Uuid) -> Result<bool> {
        let client_session = self
            .sessions
            .get(session_id)
            .ok_or(Error::InvalidSession { uid: *session_id })?;

        let Some(stream_session) = client_session.stream_session.as_ref() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        Ok(stream_session.data_plane.is_some())
    }

    pub(crate) fn display_known(&self, display_id: u32) -> bool {
        self.displays.iter().any(|d| d.id == display_id)
    }

    pub(crate) fn state_machine_proxy(&self) -> Result<StateMachineProxy> {
        self.state_machine_proxy
            .clone()
            .ok_or(Error::NotStateMachineProxy)
    }
}
