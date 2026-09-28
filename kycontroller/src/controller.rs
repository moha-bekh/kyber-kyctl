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
use std::sync::{Arc, Mutex};

use actix_web::web;
use kymux::kyproto;
use log::{info, warn};
use time::OffsetDateTime;
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use uuid::Uuid;

use kyavservice_types::Display;
use libkypc::process::ProcessFactory;

use crate::auth;
use crate::avservice;
use crate::avservices_manager::{
    AV_CLEANUP_TIMEOUT, AVServicesManager, AudioSubscription, VideoSubscription,
};
use crate::config;
use crate::error::{Error, Result};
use crate::host_events;
use crate::inputservice;
use crate::kymux_controller::{KymuxBackend, SharedKymuxInfra};
use crate::session::ClientAuthTokens;
use crate::statemachine::{self, StateMachineProxy};
use crate::tls;
use crate::utils::Task;

use crate::tray::{TrayHandle, TrayStatus};

#[allow(unused)]
pub(crate) enum DataPlaneBackend {
    Rtp,
    Kymux(KymuxBackend),
}

pub(crate) struct StreamingSession {
    pub(crate) host_event_sender: host_events::HostEventTask,
    pub(crate) data_plane: Option<DataPlaneBackend>,
    /// Pending Kymux connection receiver (between start_mux and start_session).
    /// This field holds the oneshot receiver while waiting for the client to connect.
    /// Once the connection is established, this is consumed and a KymuxBackend is created.
    pub(crate) pending_kymux_rx: Option<oneshot::Receiver<kyproto::Connection>>,
}

impl StreamingSession {
    pub(crate) fn new(host_event_sender: host_events::HostEventTask) -> Self {
        Self {
            host_event_sender,
            data_plane: None,
            pending_kymux_rx: None,
        }
    }
}

/// Why a client session ended.
#[derive(Debug, Copy, Clone)]
pub(crate) enum SessionEndReason {
    ConnectionLost,
    UserLogout,
    Expired,
}

/// A step in a client session's life, as reported to kyhub.
#[derive(Debug, Copy, Clone)]
pub(crate) enum SessionStep {
    Started,
    Reconnected,
    Ended(SessionEndReason),
}

pub(crate) struct ClientSession {
    pub(crate) uid: Uuid,
    pub(crate) username: String,
    #[cfg(feature = "kyhub")]
    pub(crate) sub: Option<String>,
    /// Set when the control plane first attaches. A reconnect keeps it.
    #[cfg(feature = "kyhub")]
    pub(crate) started_at: Option<OffsetDateTime>,
    pub(crate) expiration: Option<OffsetDateTime>,
    pub(crate) auth_tokens: ClientAuthTokens,

    pub session: Option<StreamingSession>,
    pub services: Arc<AsyncMutex<StreamingServices>>,
}

impl ClientSession {
    /// The subject claim, or the username when the token has none.
    #[cfg(feature = "kyhub")]
    pub(crate) fn user_sub(&self) -> &str {
        self.sub.as_deref().unwrap_or(&self.username)
    }

    /// `user_sub` if it is a UUID. kyhub rejects a whole batch on one bad entry.
    #[cfg(feature = "kyhub")]
    pub(crate) fn kyhub_user_sub(&self) -> Option<&str> {
        let sub = self.user_sub();
        uuid::Uuid::parse_str(sub).is_ok().then_some(sub)
    }

    pub(crate) fn new(uid: Uuid, session: auth::Session) -> Self {
        // Destructured so `sub` stays read in builds that drop the field.
        #[cfg_attr(not(feature = "kyhub"), allow(unused_variables))]
        let auth::Session {
            username,
            sub,
            expiration,
        } = session;

        Self {
            uid,
            username,
            #[cfg(feature = "kyhub")]
            sub,
            #[cfg(feature = "kyhub")]
            started_at: None,
            expiration,
            auth_tokens: ClientAuthTokens::generate(),
            session: None,
            services: Default::default(),
        }
    }
}

#[cfg(all(test, feature = "kyhub"))]
mod kyhub_user_sub_tests {
    use super::*;

    fn a_session(sub: Option<&str>) -> ClientSession {
        ClientSession::new(
            Uuid::new_v4(),
            auth::Session {
                username: "operator".to_owned(),
                sub: sub.map(str::to_owned),
                expiration: None,
            },
        )
    }

    #[test]
    fn a_uuid_subject_claim_is_kept() {
        let sub = Uuid::new_v4().to_string();
        assert_eq!(a_session(Some(&sub)).kyhub_user_sub(), Some(sub.as_str()));
    }

    #[test]
    fn a_non_uuid_subject_claim_is_dropped() {
        assert_eq!(a_session(Some("alice@example.com")).kyhub_user_sub(), None);
    }

    #[test]
    fn no_subject_claim_falls_back_to_the_username_and_is_dropped() {
        assert_eq!(a_session(None).kyhub_user_sub(), None);
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
    #[expect(unused)]
    pub(crate) audio: Option<avservice::Handler>,
    pub(crate) input: Option<inputservice::Handler>,
    pub(crate) clock_sync: Option<Task>,
    pub(crate) metrics: Option<Task>,
    /// Kydup output handles for this client (Kymux/WS backends).
    /// Dropping clears the handles, which aborts the output tasks automatically.
    pub(crate) tasks: Vec<Task>,
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
        drop(self.tasks);

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
    pub(crate) kycom: kymux::kycom::KyCom,

    // Shared AV services: avservers and duplicators shared across clients
    pub(crate) av_services_manager: AsyncMutex<AVServicesManager>,
}

impl Controller {
    pub(crate) fn new(
        process_factory: Arc<ProcessFactory>,
        config: config::Config,
        tls_config: tls::Config,
        kycom: kymux::kycom::KyCom,
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
            kycom,
            av_services_manager: Default::default(),
        });
        let state_machine = statemachine::run(this.clone(), events_rx);
        (this, state_machine)
    }

    /// Report a session step to kyhub. Not wired to a reporter yet.
    pub(crate) fn report_session(&self, _step: SessionStep, _session: &ClientSession) {}

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
            .filter(|s| s.session.is_some())
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

    pub fn session_services(&self, uid: &uuid::Uuid) -> Result<Arc<AsyncMutex<StreamingServices>>> {
        let mut sessions = self.sessions.lock().unwrap();
        let client_session = sessions
            .get_mut(uid)
            .ok_or(Error::InvalidSession { uid: *uid })?;
        Ok(client_session.services.clone())
    }

    pub fn with_session<T>(
        &self,
        uid: &uuid::Uuid,
        f: impl FnOnce(&mut ClientSession) -> Result<T>,
    ) -> Result<T> {
        let mut sessions = self.sessions.lock().unwrap();
        let client_session = sessions
            .get_mut(uid)
            .ok_or(Error::InvalidSession { uid: *uid })?;
        f(client_session)
    }

    pub fn for_each_session(&self, f: impl FnMut(&mut ClientSession)) {
        self.sessions.lock().unwrap().values_mut().for_each(f);
    }

    pub fn insert_session(&self, session: ClientSession) {
        self.sessions.lock().unwrap().insert(session.uid, session);
        self.update_tray_status();
    }

    /// Tear down the shared infra once the last session is gone. Sessions are
    /// re-read under the infra guard rather than snapshotted by the caller:
    /// `start_mux` creates/reuses the infra under this same lock.
    async fn cleanup_removed_session(&self) {
        let infra = {
            let mut infra_guard = self.shared_kymux_infra.lock().await;
            if self.sessions.lock().unwrap().is_empty() {
                infra_guard.take()
            } else {
                None
            }
        };

        if let Some(infra) = infra
            && let Err(err) = infra.stop().await
        {
            warn!("Failed to stop shared Kymux infrastructure: {err:?}");
        }

        self.update_tray_status();
    }

    /// Drain and decrement the AV subscriptions of `services`: entries reaching
    /// zero leave the maps before this returns, so a fast reconnect never
    /// joins a doomed entry. The stops themselves run detached.
    async fn release_av_services(&self, services: &Arc<AsyncMutex<StreamingServices>>) {
        let (videos, audio) = {
            let mut services = services.lock().await;
            (
                mem::take(&mut services.video_subscriptions),
                services.audio_subscription.take(),
            )
        };
        if videos.is_empty() && audio.is_none() {
            return;
        }

        // Lock order: services then av_services_manager, same as set_bitrate.
        let cleanup = self
            .av_services_manager
            .lock()
            .await
            .release_all(videos, audio);
        if cleanup.is_empty() {
            // Every released stream still has other subscribers: nothing to stop.
            return;
        }
        tokio::spawn(async move {
            if tokio::time::timeout(AV_CLEANUP_TIMEOUT, cleanup.run())
                .await
                .is_err()
            {
                warn!(
                    "AV release cleanup exceeded {AV_CLEANUP_TIMEOUT:?}; stops continue detached"
                );
            }
        });
    }

    /// Remove a session.
    pub(crate) async fn remove_session(
        &self,
        session_id: Uuid,
        reason: SessionEndReason,
    ) -> Result<()> {
        let removed = self
            .sessions
            .lock()
            .unwrap()
            .remove(&session_id)
            .ok_or(Error::InvalidSession { uid: session_id })?;

        // kyhub only knows sessions that reached streaming.
        if removed.session.is_some() {
            self.report_session(SessionStep::Ended(reason), &removed);
        }

        // release_all empties the manager maps synchronously; the session
        // itself drops outside the sessions lock.
        self.release_av_services(&removed.services).await;
        drop(removed);

        self.cleanup_removed_session().await;

        Ok(())
    }

    /// Remove multiple sessions.
    pub(crate) async fn remove_sessions_if(
        &self,
        mut f: impl FnMut(&ClientSession) -> bool,
        reason: SessionEndReason,
    ) {
        let removed = {
            let mut sessions = self.sessions.lock().unwrap();

            let uids: Vec<Uuid> = sessions
                .iter()
                .filter(|(_, session)| f(session))
                .map(|(uid, _)| *uid)
                .collect();
            uids.iter()
                .filter_map(|uid| sessions.remove(uid))
                .collect::<Vec<ClientSession>>()
        };

        for session in &removed {
            if session.session.is_some() {
                self.report_session(SessionStep::Ended(reason), session);
            }
            self.release_av_services(&session.services).await;
        }
        drop(removed);

        self.cleanup_removed_session().await;
    }

    /// Extract streaming resources for cleanup outside the lock.
    ///
    /// The session remains in the map but with empty services.
    pub(crate) async fn stop_streaming(&self, session_id: &Uuid) -> Result<()> {
        info!("Stop streaming for session {}", session_id);

        let services = self.with_session(session_id, |s| {
            if let Some(session) = &mut s.session {
                session.data_plane = None;
            }

            Ok(s.services.clone())
        })?;

        self.release_av_services(&services).await;

        if let Err(err) = mem::take(&mut *services.lock().await).stop().await {
            warn!("Failed to stop streaming services: {err:?}");
        }

        Ok(())
    }

    pub(crate) fn has_data_plane(&self, session_id: &Uuid) -> Result<bool> {
        let sessions = self.sessions.lock().unwrap();
        let client_session = sessions
            .get(session_id)
            .ok_or(Error::InvalidSession { uid: *session_id })?;

        let Some(stream_session) = client_session.session.as_ref() else {
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
