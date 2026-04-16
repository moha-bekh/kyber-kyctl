// Project Kyber: statemachine.rs
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

use actix_web::web;
use log::{info, warn};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::avservices_manager::StreamId;
use crate::utils::{Task, TypeConvert};
use crate::{avservice, host_events};
use crate::{Controller, HostEvent};

/// Route a stream event (PipelineEvent or Stopped) emitted by an
/// AVServicesManager handler to exactly the sessions subscribed to that
/// stream, matched by the subscription's registry key.
///
/// Several streams may target the same display (mono multi-session, or an
/// exclusive stream alongside a shared one); matching by stream identity
/// rather than by display keeps each session's events isolated.
fn route_to_subscribers(ctrl: &Controller, stream_id: &StreamId, event: &HostEvent) {
    for session in ctrl.sessions.values() {
        let Some(streaming_session) = &session.stream_session else {
            continue;
        };
        let matches = match stream_id {
            StreamId::Video(key) => session
                .services
                .video_subscriptions
                .iter()
                .any(|sub| sub.key() == key),
            StreamId::Audio(key) => session
                .services
                .audio_subscription
                .as_ref()
                .is_some_and(|sub| sub.key() == key),
        };
        if matches {
            streaming_session.host_event_sender.send(event.clone());
        }
    }
}

#[derive(Debug)]
pub(crate) enum StateMachineEvent {
    AvService {
        /// `Some` only for the legacy per-session RTP handlers (routed to that
        /// session). `None` for AVServicesManager and global handlers, which
        /// are routed by `stream_id` instead.
        session_id: Option<Uuid>,
        /// Routing identity of the emitting stream. `Some` for the
        /// shared/exclusive AVServicesManager streams (routed to the sessions
        /// subscribed to this exact stream); `None` for RTP (routed by
        /// `session_id`) and global enumerate/SDP handlers (broadcast).
        stream_id: Option<StreamId>,
        event: kyavservice_types::Event,
    },
    AvServiceStopped {
        /// See `AvService::session_id`.
        session_id: Option<Uuid>,
        /// See `AvService::stream_id`.
        stream_id: Option<StreamId>,
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
    pub fn new(events_tx: mpsc::Sender<StateMachineEvent>) -> Self {
        Self { events_tx }
    }

    pub(crate) async fn send(&self, msg: StateMachineEvent) {
        let ret = self.events_tx.send(msg).await;
        if let Err(err) = ret {
            warn!("Failed to send event {err:?}");
        }
    }
}

pub(crate) fn run(
    ctrl: web::Data<Mutex<Controller>>,
    events_rx: mpsc::Receiver<StateMachineEvent>,
) -> Task {
    Task::spawn(StateMachine::process_events(ctrl, events_rx))
}

// Dummy impl block to avoid a huge diff and annoying conflicts
struct StateMachine;
impl StateMachine {
    #[allow(clippy::single_match)]
    async fn process_events(
        ctrl: web::Data<Mutex<Controller>>,
        mut events_rx: mpsc::Receiver<StateMachineEvent>,
    ) {
        while let Some(evt) = events_rx.recv().await {
            match evt {
                StateMachineEvent::AvService {
                    session_id,
                    stream_id: _,
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
                                .send(HostEvent::DisplayList {
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
                // AVServicesManager stream (shared or exclusive): route to
                // every session subscribed to this exact stream.
                StateMachineEvent::AvService {
                    stream_id: Some(stream_id),
                    event: kyavservice_types::Event::PipelineEvent(event),
                    ..
                } => {
                    let ctrl = ctrl.lock().await;
                    let host_event = HostEvent::StreamerPipelineEvent(event.into());
                    route_to_subscribers(&ctrl, &stream_id, &host_event);
                }
                // Legacy per-session avservice (RTP): send to that session only.
                StateMachineEvent::AvService {
                    session_id: Some(sid),
                    stream_id: None,
                    event: kyavservice_types::Event::PipelineEvent(event),
                } => {
                    let ctrl = ctrl.lock().await;
                    let host_event = HostEvent::StreamerPipelineEvent(event.into());
                    if let Some(session) = ctrl.sessions.get(&sid) {
                        if let Some(streaming_session) = &session.stream_session {
                            streaming_session.host_event_sender.send(host_event);
                        }
                    }
                }
                StateMachineEvent::AvServiceStopped {
                    session_id,
                    stream_id,
                    type_,
                } => {
                    info!("AvServer {type_:?} stopped (session: {:?})", session_id);

                    let pipeline_type = match type_ {
                        avservice::HandlerType::EnumerateDisplays => {
                            host_events::StreamerPipelineType::EnumerateDisplays
                        }
                        avservice::HandlerType::VideoSdp => continue,
                        avservice::HandlerType::Video => host_events::StreamerPipelineType::Video,
                        avservice::HandlerType::Audio => host_events::StreamerPipelineType::Audio,
                    };

                    let event =
                        HostEvent::StreamerPipelineEvent(host_events::StreamerPipelineEvent {
                            type_: pipeline_type,
                            stage: host_events::StreamerPipelineStage::Process,
                            event: host_events::StreamerPipelineEventType::Stopped,
                        });

                    let ctrl = ctrl.lock().await;

                    match (stream_id, session_id) {
                        // AVServicesManager stream (shared or exclusive): reach
                        // the sessions subscribed to this exact stream.
                        (Some(stream_id), _) => {
                            route_to_subscribers(&ctrl, &stream_id, &event);
                        }
                        // Legacy per-session (RTP): notify that session only.
                        (None, Some(sid)) => {
                            if let Some(session) = ctrl.sessions.get(&sid) {
                                if let Some(streaming_session) = &session.stream_session {
                                    streaming_session.host_event_sender.send(event);
                                }
                            }
                        }
                        // Global handler (e.g. watchdog display enumeration):
                        // broadcast to every session.
                        (None, None) => {
                            for session in ctrl.sessions.values() {
                                if let Some(streaming_session) = &session.stream_session {
                                    streaming_session.host_event_sender.send(event.clone());
                                }
                            }
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
                            Ok(cleanup) => {
                                cleanup.run().await;
                                ctrl.lock().await.update_tray_status().await;
                            }
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
