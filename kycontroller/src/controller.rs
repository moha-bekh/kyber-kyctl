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
use std::sync::{atomic, Arc, Mutex};

use actix_web::web;
use kymux::kyproto;
use log::{info, warn};
use time::OffsetDateTime;
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use uuid::Uuid;

use kyavservice_types::Display;
use libkypc::process::ProcessFactory;

use crate::auth;
use crate::avservice;
use crate::avservices_manager::{AVServicesManager, AudioSubscription, VideoSubscription};
use crate::config;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::kymux_controller::{KymuxBackend, SharedKymuxInfra};
use crate::session::ClientAuthTokens;
use crate::statemachine::{self, StateMachineProxy};
use crate::tls;
use crate::utils::Task;
use crate::ws::WebsocketBackend;
use crate::HostEventSender;

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
}

pub(crate) struct ClientSession {
    pub(crate) uid: Uuid,
    pub(crate) username: String,
    pub(crate) expiration: Option<OffsetDateTime>,
    pub(crate) auth_tokens: ClientAuthTokens,

    // These resources are asynchronous, so they have their own async mutex.
    // The `Arc` enables doing this outside the parent mutex.
    pub(crate) stream: Arc<AsyncMutex<Stream>>,
}

#[derive(Default)]
pub(crate) struct Stream {
    pub session: Option<StreamingSession>,
    pub services: StreamingServices,
}

impl ClientSession {
    pub(crate) fn new(uid: Uuid, session: auth::Session) -> Self {
        Self {
            uid,
            username: session.username,
            expiration: session.expiration,
            auth_tokens: ClientAuthTokens::generate(),
            stream: Default::default(),
        }
    }

    async fn stop(self) {
        info!("Stop session {}", self.uid);

        let stream = mem::take(&mut *self.stream.lock().await);

        if let Err(err) = stream.services.stop().await {
            warn!("Failed to stop streaming services: {err:?}");
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
    pub(crate) output_handles: Vec<Task>,
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

    pub(crate) displays: Mutex<Vec<Display>>,

    pub(crate) config: config::Config,
    pub(crate) tls_config: tls::Config,

    pub(crate) sessions: Mutex<HashMap<Uuid, ClientSession>>,

    pub(crate) state_machine_proxy: StateMachineProxy,

    /// Shared Kymux infrastructure (unified server supporting both QUIC and WebTransport on same port)
    /// `None` if not running
    pub(crate) shared_kymux_infra: AsyncMutex<Option<SharedKymuxInfra>>,
    tray_handle: Mutex<Option<TrayHandle>>,

    // Shared kycom instance for all backends (IPC bridge to avserver/inputserver processes)
    pub(crate) kycom: AsyncMutex<Option<kymux::kycom::KyCom>>,
    /// Counter for allocating kycom endpoint IDs (WS backend; Kymux uses kyproto's allocator)
    pub(crate) next_ws_endpoint_id: atomic::AtomicU16,

    // Shared AV services: avservers and duplicators shared across clients
    /// Must always be taken after `kycom`.
    pub(crate) av_services_manager: AsyncMutex<AVServicesManager>,
}

impl Controller {
    pub(crate) fn new(
        process_factory: Arc<ProcessFactory>,
        config: config::Config,
        tls_config: tls::Config,
    ) -> (web::Data<Self>, Task) {
        let (events_tx, events_rx) = tokio::sync::mpsc::channel(32);

        let this = web::Data::new(Self {
            process_factory,
            displays: Mutex::new(vec![]),
            config,
            tls_config,
            sessions: Mutex::new(HashMap::new()),
            state_machine_proxy: StateMachineProxy::new(events_tx),
            shared_kymux_infra: Default::default(),
            tray_handle: Mutex::new(None),
            kycom: Default::default(),
            next_ws_endpoint_id: atomic::AtomicU16::new(0x8000),
            av_services_manager: Default::default(),
        });
        let state_machine = statemachine::run(this.clone(), events_rx);
        (this, state_machine)
    }

    /// Set the tray handle for status updates.
    pub(crate) fn set_tray_handle(&self, handle: TrayHandle) {
        *self.tray_handle.lock().unwrap() = Some(handle);
    }

    /// Signal the Windows service to stop.
    ///
    /// Delegates to the tray handle if available; no-op otherwise.
    pub(crate) fn signal_service_stop(&self) {
        if let Some(tray) = &*self.tray_handle.lock().unwrap() {
            tray.signal_service_stop();
        }
    }

    /// Update the system tray status.
    pub fn update_tray_status(&self) {
        let clients = self
            .sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.username.clone())
            .collect();

        if let Some(tray) = &mut *self.tray_handle.lock().unwrap() {
            tray.set_status(TrayStatus { clients });
        };
    }

    /// Shutdown the system tray (removes the icon from taskbar).
    pub fn shutdown_tray(&self) {
        if let Some(mut tray) = self.tray_handle.lock().unwrap().take() {
            tray.shutdown();
        }
    }

    /// Ports for the shared kycom instance.
    pub(crate) const KYCOM_PORTS: std::ops::Range<u16> = 9091..9110;

    /// Allocate a kycom endpoint ID for WS backend channels.
    pub(crate) fn alloc_ws_endpoint_id(&self) -> u16 {
        self.next_ws_endpoint_id
            .fetch_add(1, atomic::Ordering::Relaxed)
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
    pub(crate) async fn try_take_idle_kycom(&self) -> Option<kymux::kycom::KyCom> {
        let no_session = self.sessions.lock().unwrap().is_empty();
        let no_streams = {
            let av_services_manager = self.av_services_manager.lock().await;
            av_services_manager.video_streams.is_empty()
                && av_services_manager.audio_streams.is_empty()
        };
        if no_session && no_streams {
            self.kycom.lock().await.take()
        } else {
            None
        }
    }

    pub fn get_stream_session(&self, uid: &uuid::Uuid) -> Result<Arc<AsyncMutex<Stream>>> {
        let sessions = self.sessions.lock().unwrap();
        let client_session = sessions
            .get(uid)
            .ok_or(Error::InvalidSession { uid: *uid })?;
        Ok(client_session.stream.clone())
    }

    pub fn get_all_stream_sessions(&self) -> Vec<Arc<AsyncMutex<Stream>>> {
        self.sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.stream.clone())
            .collect()
    }

    pub fn insert_session(&self, session: ClientSession) {
        self.sessions.lock().unwrap().insert(session.uid, session);
    }

    async fn cleanup_removed_session(&self, no_sessions: bool) {
        if no_sessions {
            if let Some(infra) = self.shared_kymux_infra.lock().await.take() {
                if let Err(err) = infra.stop().await {
                    warn!("Failed to stop shared Kymux infrastructure: {err:?}");
                }
            }

            let no_streams = {
                let av_services_manager = self.av_services_manager.lock().await;
                av_services_manager.video_streams.is_empty()
                    && av_services_manager.audio_streams.is_empty()
            };
            if no_streams {
                *self.kycom.lock().await = None;
            }
        }

        self.update_tray_status();
    }

    /// Remove a session.
    pub(crate) async fn remove_session(&self, session_id: Uuid) -> Result<()> {
        let session;
        let no_sessions;

        {
            let mut sessions = self.sessions.lock().unwrap();

            session = sessions
                .remove(&session_id)
                .ok_or(Error::InvalidSession { uid: session_id })?;
            no_sessions = sessions.is_empty();
        }

        session.stop().await;

        self.cleanup_removed_session(no_sessions).await;

        Ok(())
    }

    /// Remove multiple sessions.
    pub(crate) async fn remove_sessions_if(&self, mut f: impl FnMut(&ClientSession) -> bool) {
        let removed: Vec<_>;
        let no_sessions;

        {
            let mut sessions = self.sessions.lock().unwrap();

            removed = sessions
                .extract_if(|_, session| f(session))
                .map(|(_, s)| s)
                .collect();
            no_sessions = sessions.is_empty();
        }

        for session in removed {
            session.stop().await;
        }

        self.cleanup_removed_session(no_sessions).await;
    }

    /// Extract streaming resources for cleanup outside the lock.
    ///
    /// The session remains in the map but with empty services.
    pub(crate) async fn remove_streaming(&self, session_id: &Uuid) -> Result<()> {
        info!("Stop streaming for session {}", session_id);

        let stream = self.get_stream_session(session_id)?;
        let mut stream = stream.lock().await;

        if let Some(session) = &mut stream.session {
            session.data_plane = None;
        }
        if let Err(err) = mem::take(&mut stream.services).stop().await {
            warn!("Failed to stop streaming services: {err:?}");
        }

        Ok(())
    }

    pub(crate) async fn has_data_plane(&self, session_id: &Uuid) -> Result<bool> {
        let stream = self.get_stream_session(session_id)?;
        let stream = stream.lock().await;

        let Some(stream_session) = stream.session.as_ref() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        // Data plane is considered active if either:
        // - An established connection exists (data_plane is Some)
        // - A pending connection is waiting (pending_kymux_rx is Some)
        Ok(stream_session.data_plane.is_some() || stream_session.pending_kymux_rx.is_some())
    }

    pub(crate) fn display_known(&self, display_id: u32) -> bool {
        self.displays
            .lock()
            .unwrap()
            .iter()
            .any(|d| d.id == display_id)
    }
}
