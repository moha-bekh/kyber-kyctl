// Project Kyber: controller.rs
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

use std::collections::HashMap;
use std::mem;
use std::sync::Arc;

use kymux::kyproto;
use log::{info, warn};
use time::OffsetDateTime;
use tokio::sync::oneshot;
use uuid::Uuid;

use kyavservice_types::Display;
use libkypc::process::ProcessFactory;

use crate::auth;
use crate::avservice;
use crate::avservices_manager::{AVServicesManager, AudioSubscription, VideoSubscription};
use crate::config;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::kydup;
use crate::kymux_controller::{KymuxBackend, SharedKymuxInfra};
use crate::session::ClientAuthTokens;
use crate::tls;
use crate::utils::Task;
use crate::ws::WebsocketBackend;
use crate::HostEventSender;
use crate::StateMachineProxy;

use crate::tray::{TrayHandle, TrayStatus};

#[allow(unused)]
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
            Some(DataPlaneBackend::Kymux(_)) => {}
            Some(DataPlaneBackend::Websocket(backend)) => backend.stop().await?,
            None => {}
        }

        Ok(())
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        info!("Stop dataplane");
        self.stop_dataplane().await?;

        Ok(())
    }
}

/// Cleanup data for a fully removed session.
///
/// Holds all resources that need async teardown. Must be used OUTSIDE the
/// Controller lock to avoid blocking concurrent requests.
pub(crate) struct SessionCleanup {
    session: ClientSession,
    should_stop_infra: bool,
    infra: Arc<tokio::sync::Mutex<SharedKymuxInfra>>,
    /// Idle kycom taken under the lock; stopped here so its TCP listener
    /// teardown doesn't block other endpoints on the Controller mutex.
    kycom_to_stop: Option<kymux::kycom::KyCom>,
}

impl SessionCleanup {
    pub(crate) async fn run(mut self) {
        info!("Stop session {}", self.session.uid);

        if let Some(stream_session) = self.session.stream_session.take() {
            if let Err(err) = stream_session.stop().await {
                warn!("Failed to stop client session: {err:?}");
            }
        }

        if let Err(err) = self.session.services.stop().await {
            warn!("Failed to stop streaming services: {err:?}");
        }

        drop(self.kycom_to_stop);

        if self.should_stop_infra {
            if let Err(err) = self.infra.lock().await.stop().await {
                warn!("Failed to stop shared Kymux infrastructure: {err:?}");
            }
        }
    }
}

/// Cleanup data for stopping streaming without removing the session.
pub(crate) struct StreamingCleanup {
    data_plane: Option<DataPlaneBackend>,
    services: StreamingServices,
}

impl StreamingCleanup {
    pub(crate) async fn run(mut self) {
        match self.data_plane.take() {
            Some(DataPlaneBackend::Websocket(backend)) => {
                if let Err(err) = backend.stop().await {
                    warn!("Failed to stop websocket backend: {err:?}");
                }
            }
            _ => {}
        }

        if let Err(err) = self.services.stop().await {
            warn!("Failed to stop streaming services: {err:?}");
        }
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
    /// Per-session video avservers, used by the legacy RTP backend only.
    /// Kymux/WS backends route their video/audio streams through [`AVServicesManager`]
    /// regardless of mode, so `video` stays empty for them and teardown is
    /// handled via [`Self::video_subscriptions`].
    pub(crate) video: Vec<avservice::Handler>,
    /// Per-session audio avserver, RTP only, same rationale as `video`.
    pub(crate) audio: Option<avservice::Handler>,
    pub(crate) input: Option<inputservice::Handler>,
    pub(crate) clock_sync: Option<Task>,
    pub(crate) metrics: Option<Task>,
    /// Kydup output handles for this client (Kymux/WS backends).
    /// Dropping clears the handles, which aborts the output tasks automatically.
    pub(crate) output_handles: Vec<kydup::OutputHandle>,
    /// RAII subscriptions to [`AVServicesManager`] video entries (Kymux/WS backends).
    /// Dropping triggers async decrement via the AV reaper, which stops the
    /// avservice when the last subscription leaves; covers both `Shared` and
    /// `Exclusive` entries.
    pub(crate) video_subscriptions: Vec<VideoSubscription>,
    /// RAII subscription to an [`AVServicesManager`] audio entry. See `video_subscriptions`.
    pub(crate) audio_subscription: Option<AudioSubscription>,
}

impl StreamingServices {
    async fn stop(self) -> Result<()> {
        // OutputHandle::drop aborts the task automatically. Kymux/WS avservices
        // are stopped by the AV reaper after the subscriptions held here
        // drop; see AVServicesManager. RTP avservices (in `video`/`audio`) are
        // per-session and stopped synchronously below.
        drop(self.output_handles);

        for service in self.video {
            info!("Stop running video AVServer");
            service.stop().await?;
        }

        if let Some(audio) = self.audio {
            info!("Stop running audio AVServer");
            audio.stop().await?;
        }

        if let Some(input) = self.input {
            info!("Stop running InputServer");
            input.stop().await?;
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
    pub(crate) tray_handle: Option<TrayHandle>,

    // Shared kycom instance for all backends (IPC bridge to avserver/inputserver processes)
    pub(crate) kycom: Option<kymux::kycom::KyCom>,
    /// Counter for allocating kycom endpoint IDs (WS backend; Kymux uses kyproto's allocator)
    pub(crate) next_ws_endpoint_id: u16,

    // Shared AV services: avservers and duplicators shared across clients
    pub(crate) av_services_manager: AVServicesManager,
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
            tray_handle: None,
            kycom: None,
            next_ws_endpoint_id: 0x8000,
            av_services_manager: Default::default(),
        }
    }

    /// Set the tray handle for status updates.
    pub(crate) fn set_tray_handle(&mut self, handle: TrayHandle) {
        self.tray_handle = Some(handle);
    }

    /// Signal the Windows service to stop.
    ///
    /// Delegates to the tray handle if available; no-op otherwise.
    pub(crate) fn signal_service_stop(&self) {
        if let Some(ref tray) = self.tray_handle {
            tray.signal_service_stop();
        }
    }

    /// Update the system tray status.
    pub(crate) async fn update_tray_status(&mut self) {
        let Some(ref tray) = self.tray_handle else {
            return;
        };

        let clients: Vec<String> = self.sessions.values().map(|s| s.username.clone()).collect();

        tray.set_status(TrayStatus { clients }).await;
    }

    /// Shutdown the system tray (removes the icon from taskbar).
    pub(crate) async fn shutdown_tray(&mut self) {
        if let Some(mut tray) = self.tray_handle.take() {
            tray.shutdown().await;
        }
    }

    /// Ports for the shared kycom instance.
    const KYCOM_PORTS: std::ops::Range<u16> = 9091..9110;

    /// Start the shared kycom if not already running.
    pub(crate) async fn ensure_kycom(&mut self) -> Result<()> {
        if self.kycom.is_none() {
            self.kycom = Some(kymux::kycom::KyCom::start_on_any_port(Self::KYCOM_PORTS).await?);
        }
        Ok(())
    }

    /// Allocate a kycom endpoint ID for WS backend channels.
    pub(crate) fn alloc_ws_endpoint_id(&mut self) -> u16 {
        let id = self.next_ws_endpoint_id;
        self.next_ws_endpoint_id = self.next_ws_endpoint_id.wrapping_add(1);
        id
    }

    /// Take ownership of the shared kycom iff no sessions and no AV streams
    /// remain; the moment kycom can no longer be in use. The caller stops
    /// it outside the Controller lock.
    ///
    /// Called from two sites:
    ///   * the AV reaper, right after `AVCleanup::run()` has fully stopped
    ///     the last avservers via libkypc; the normal path when a session
    ///     drops its subscriptions;
    ///   * `take_session`, for the path where the session never had (or
    ///     already lost) its subscriptions, so the reaper wouldn't otherwise
    ///     be woken to clean up kycom.
    pub(crate) fn try_take_idle_kycom(&mut self) -> Option<kymux::kycom::KyCom> {
        if self.sessions.is_empty()
            && self.av_services_manager.video_streams.is_empty()
            && self.av_services_manager.audio_streams.is_empty()
        {
            self.kycom.take()
        } else {
            None
        }
    }

    pub fn get_session<'a>(&'a mut self, uid: &uuid::Uuid) -> Result<&'a mut ClientSession> {
        self.sessions
            .get_mut(uid)
            .ok_or(Error::InvalidSession { uid: *uid })
    }

    /// Remove a session and prepare cleanup data.
    ///
    /// Returns a `SessionCleanup` that must be `.run().await`ed AFTER releasing
    /// the Controller lock to avoid blocking concurrent requests like `/login`.
    pub(crate) fn take_session(&mut self, session_id: Uuid) -> Result<SessionCleanup> {
        info!("Preparing stop session {}", session_id);

        let session = self
            .sessions
            .remove(&session_id)
            .ok_or(Error::InvalidSession { uid: session_id })?;

        // Subscriptions on session.services.video_subscriptions /
        // audio_subscription will drop when SessionCleanup.run() drops the
        // ClientSession outside the lock, signalling the reaper to decrement
        // the shared AV client_counts asynchronously. The reaper stops each
        // shared avservice once its client_count hits 0; no need to stop
        // them synchronously here (which would block the Controller lock).
        //
        // kycom teardown is deferred to the reaper too, so the avservers
        // receive their libkypc Stop before their IPC TCP sockets disappear.
        // We only stop kycom here in the corner case where the session had
        // no subscriptions left to drop (e.g. it logged out after explicitly
        // stopping streaming); without that, no reaper message would fire
        // and kycom would leak until the next start_session.
        let should_stop_infra = self.sessions.is_empty();
        let kycom_to_stop = self.try_take_idle_kycom();

        let infra = self.shared_kymux_infra.clone();

        Ok(SessionCleanup {
            session,
            should_stop_infra,
            infra,
            kycom_to_stop,
        })
    }

    /// Extract streaming resources for cleanup outside the lock.
    ///
    /// The session remains in the map but with empty services.
    pub(crate) fn take_streaming(&mut self, session_id: Uuid) -> Result<StreamingCleanup> {
        info!("Stop streaming for session {}", session_id);

        let client_session = self
            .sessions
            .get_mut(&session_id)
            .ok_or(Error::InvalidSession { uid: session_id })?;

        let data_plane = client_session
            .stream_session
            .as_mut()
            .and_then(|s| s.data_plane.take());

        // mem::take moves the subscriptions out with the rest of the services.
        // They drop (and signal the reaper) when StreamingCleanup.run() drops
        // the services outside the Controller lock.
        let services = mem::take(&mut client_session.services);

        Ok(StreamingCleanup {
            data_plane,
            services,
        })
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
