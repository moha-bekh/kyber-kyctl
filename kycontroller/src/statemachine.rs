// Project Kyber: statemachine.rs
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

use actix_web::web;
use log::{info, warn};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    task,
};
use uuid::Uuid;

use crate::utils::TypeConvert;
use crate::{avservice, host_events};
use crate::{Controller, Error, HostEvent, Result};

#[derive(Debug)]
pub(crate) enum StateMachineEvent {
    AvService {
        /// None means the event targets all sessions (e.g., from watchdog)
        session_id: Option<Uuid>,
        event: kyavservice_types::Event,
    },
    AvServiceStopped {
        /// None means global (e.g., watchdog stopped)
        session_id: Option<Uuid>,
        type_: avservice::HandlerType,
    },
    HostEvents {
        session_id: Uuid,
        notification: host_events::Notification,
    },
    KymuxTimeout {
        session_id: Uuid,
    },
}

#[derive(Clone)]
pub(crate) struct StateMachineProxy {
    events_tx: mpsc::Sender<StateMachineEvent>,
}

impl StateMachineProxy {
    pub(crate) async fn send(&self, msg: StateMachineEvent) {
        let ret = self.events_tx.send(msg).await;
        if let Err(err) = ret {
            warn!("Failed to send event {err:?}");
        }
    }
}

pub(crate) struct StateMachine {
    events_tx: mpsc::Sender<StateMachineEvent>,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl StateMachine {
    pub(crate) fn new(ctrl: web::Data<Mutex<Controller>>) -> Result<Self> {
        let (events_tx, events_rx) = mpsc::channel(32);
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn_local(Self::task_entry(ctrl, events_rx, stop_rx));

        Ok(Self {
            events_tx,
            handle,
            stop_tx,
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("Statemachine task stop send request failed");
        }

        self.handle.await.map_err(|err| {
            warn!("Statemachine task panicked: {err:?}");
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    pub(crate) fn create_proxy(&self) -> StateMachineProxy {
        StateMachineProxy {
            events_tx: self.events_tx.clone(),
        }
    }

    async fn task_entry(
        ctrl: web::Data<Mutex<Controller>>,
        events_rx: mpsc::Receiver<StateMachineEvent>,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_events(ctrl, events_rx) => {}
        }
    }

    #[allow(clippy::single_match)]
    async fn process_events(
        ctrl: web::Data<Mutex<Controller>>,
        mut events_rx: mpsc::Receiver<StateMachineEvent>,
    ) {
        while let Some(evt) = events_rx.recv().await {
            match evt {
                StateMachineEvent::AvService {
                    session_id,
                    event: kyavservice_types::Event::DisplayListUpdated { displays },
                } => {
                    info!(
                        "Got display update (session: {:?}): {displays:?}",
                        session_id
                    );
                    let mut ctrl = ctrl.lock().await;

                    // Broadcast display list to all sessions (non-blocking to avoid
                    // holding the lock while a slow client backs up the channel)
                    for session in ctrl.sessions.values() {
                        if let Some(streaming_session) = &session.stream_session {
                            streaming_session
                                .host_event_sender
                                .try_send(HostEvent::DisplayList {
                                    displays: displays.convert(),
                                });
                        }
                    }

                    // Update inputservice(s)
                    let host_config = kynputservice_types::HostConfig {
                        display_list: displays.convert(),
                    };

                    match session_id {
                        // Global event (e.g., from watchdog): update all inputservices
                        None => {
                            for session in ctrl.sessions.values_mut() {
                                if let Some(inputservice) = session.services.input.as_mut() {
                                    if let Err(err) =
                                        inputservice.update_host_config(host_config.clone()).await
                                    {
                                        warn!("Failed to send HostConfig to InputServer: {err:?}");
                                    }
                                }
                            }
                        }
                        // Session-specific event: update only that session's inputservice
                        Some(sid) => {
                            if let Some(session) = ctrl.sessions.get_mut(&sid) {
                                if let Some(inputservice) = session.services.input.as_mut() {
                                    if let Err(err) =
                                        inputservice.update_host_config(host_config).await
                                    {
                                        warn!("Failed to send HostConfig to InputServer: {err:?}");
                                    }
                                }
                            }
                        }
                    }

                    ctrl.displays = displays;
                }
                StateMachineEvent::AvService {
                    session_id,
                    event: kyavservice_types::Event::PipelineEvent(event),
                } => {
                    // PipelineEvent is session-specific, ignore if no session_id
                    let Some(sid) = session_id else {
                        warn!("Received PipelineEvent without session_id, ignoring");
                        continue;
                    };

                    let ctrl = ctrl.lock().await;

                    if let Some(session) = ctrl.sessions.get(&sid) {
                        if let Some(streaming_session) = &session.stream_session {
                            streaming_session
                                .host_event_sender
                                .try_send(HostEvent::StreamerPipelineEvent(event.into()));
                        }
                    }
                }
                StateMachineEvent::AvServiceStopped { session_id, type_ } => {
                    info!("AvServer {type_:?} stopped (session: {:?})", session_id);

                    // Global AvService (e.g., watchdog) stopped - just log, no client notification
                    let Some(sid) = session_id else {
                        warn!("Global AvServer {type_:?} stopped");
                        continue;
                    };

                    let ctrl = ctrl.lock().await;

                    if let Some(session) = ctrl.sessions.get(&sid) {
                        if let Some(streaming_session) = &session.stream_session {
                            let type_ = match type_ {
                                avservice::HandlerType::EnumerateDisplays => {
                                    host_events::StreamerPipelineType::EnumerateDisplays
                                }
                                avservice::HandlerType::VideoSdp => continue,
                                avservice::HandlerType::Video => {
                                    host_events::StreamerPipelineType::Video
                                }
                                avservice::HandlerType::Audio => {
                                    host_events::StreamerPipelineType::Audio
                                }
                            };

                            let event = host_events::StreamerPipelineEvent {
                                type_,
                                stage: host_events::StreamerPipelineStage::Process,
                                event: host_events::StreamerPipelineEventType::Stopped,
                            };

                            streaming_session
                                .host_event_sender
                                .try_send(HostEvent::StreamerPipelineEvent(event));
                        }
                    }
                }
                StateMachineEvent::HostEvents {
                    session_id,
                    notification,
                } => match notification {
                    host_events::Notification::ConnectionLost => {
                        warn!("Control Websocket disconnected for session {}", session_id);

                        // Take session under lock, then stop outside to avoid
                        // blocking /login and other requests during teardown
                        let cleanup = {
                            let mut ctrl = ctrl.lock().await;
                            ctrl.take_session(session_id)
                        };

                        match cleanup {
                            Ok(cleanup) => cleanup.run().await,
                            Err(err) => {
                                warn!("Fail to stop session {}: {err:?}", session_id)
                            }
                        }
                    }
                },
                StateMachineEvent::KymuxTimeout { session_id } => {
                    warn!("Kymux connection timeout for session {}", session_id);
                }
                _ => {}
            }
        }
    }
}
