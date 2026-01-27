// Project Kyber: controller.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
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

use std::collections::HashMap;
use std::sync::Arc;

use kymux::kyproto;
use log::{info, warn};
use time::OffsetDateTime;
use tokio::sync::oneshot;
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
    /// Pending Kymux connection receiver (between start_mux and start_session).
    /// This field holds the oneshot receiver while waiting for the client to connect.
    /// Once the connection is established, this is consumed and a KymuxBackend is created.
    pub(crate) pending_kymux_rx: Option<oneshot::Receiver<Arc<kyproto::Connection>>>,
}

impl StreamingSession {
    pub(crate) fn new(host_event_sender: HostEventSender) -> Self {
        Self {
            host_event_sender,
            data_plane: None,
            pending_kymux_rx: None,
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
    // Always exists but can be in stopped or running state internally
    pub(crate) shared_kymux_infra: Arc<tokio::sync::Mutex<SharedKymuxInfra>>,
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
            shared_kymux_infra: Arc::new(tokio::sync::Mutex::new(SharedKymuxInfra::new())),
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

        // Stop shared Kymux infrastructure when no more sessions
        if self.sessions.is_empty() {
            if let Err(err) = self.shared_kymux_infra.lock().await.stop().await {
                warn!("Failed to stop shared Kymux infrastructure: {err:?}");
            }
        }

        Ok(())
    }

    pub(crate) fn has_data_plane(&self, session_id: &Uuid) -> Result<bool> {
        let client_session = self
            .sessions
            .get(session_id)
            .ok_or(Error::InvalidSession { uid: *session_id })?;

        let Some(stream_session) = client_session.stream_session.as_ref() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        // Data plane is considered active if either:
        // - An established connection exists (data_plane is Some)
        // - A pending connection is waiting (pending_kymux_rx is Some)
        Ok(stream_session.data_plane.is_some() || stream_session.pending_kymux_rx.is_some())
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
