// Project Kyber: host_events.rs
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

use std::time::Duration;

use actix_ws::{CloseCode, CloseReason};
use log::{error, info, warn};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{Error, Result, StateMachineEvent, StateMachineProxy, utils::Task};

pub(crate) use kyctl_common::host_events::{
    Display, HostEvent, HostEventFrame, StreamerPipelineEvent, StreamerPipelineEventType,
    StreamerPipelineStage, StreamerPipelineType,
};

// Naming taken from man(7) tcp: TCP_KEEPCNT/TCP_KEEPINTVL
pub(crate) const KEEPINTVL: Duration = Duration::from_secs(20);
pub(crate) const KEEPCNT: u64 = 3;

struct PingTask {
    session_id: uuid::Uuid,
    ping_rx: mpsc::Receiver<u64>,
    events_tx: mpsc::UnboundedSender<HostEventFrame>,
    statemachine_proxy: StateMachineProxy,
}

impl PingTask {
    async fn run(&mut self) {
        let mut interval = tokio::time::interval(KEEPINTVL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        const FIRST_ID: u64 = 0;

        let mut last_id_received = None;
        let mut next_id = FIRST_ID;

        loop {
            tokio::select! {
                biased;
                // Send Ping
                _ = interval.tick() => {
                    // Prepare id
                    let id = next_id;
                    next_id += 1;

                    // Check if timeout occurred
                    // - If true: Notify of disconnection lost
                    // - Else continue to ping
                    if id - last_id_received.unwrap_or(FIRST_ID) >= KEEPCNT {
                        self.statemachine_proxy.send(StateMachineEvent::HostEvents { session_id: self.session_id, notification: Notification::ConnectionLost })
                        .await;

                        break;
                    } else {
                        let msg = HostEventFrame::Ping { id };

                        let ret = self.events_tx.send(msg);
                        if let Err(err) = ret {
                            warn!("Failed to Ping({id}): {err:?}");
                            break;
                        }
                    }
                }
                // Pong received
                id = self.ping_rx.recv() => {
                    let Some(id) = id else {
                        warn!("Ping Rx channel closed");
                        break;
                    };

                    last_id_received = Some(id);
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum Notification {
    ConnectionLost,
}

pub(crate) fn convert_pipeline_event(
    event: kyavservice_types::PipelineEvent,
) -> StreamerPipelineEvent {
    let type_ = match event.type_ {
        kyavservice_types::PipelineType::Audio => StreamerPipelineType::Audio,
        kyavservice_types::PipelineType::Video => StreamerPipelineType::Video,
    };
    let stage = match event.stage {
        kyavservice_types::PipelineStage::Initialization => StreamerPipelineStage::Initialization,
        kyavservice_types::PipelineStage::Acquisition => StreamerPipelineStage::Acquisition,
        kyavservice_types::PipelineStage::Encoder => StreamerPipelineStage::Encoder,
    };
    let event = match event.event {
        kyavservice_types::PipelineEventType::Error => StreamerPipelineEventType::Error,
    };
    StreamerPipelineEvent {
        type_,
        stage,
        event,
    }
}

#[derive(Clone)]
pub(crate) struct HostEventSender {
    events_tx: mpsc::UnboundedSender<HostEventFrame>,
}

impl HostEventSender {
    pub(crate) fn send(&self, event: HostEvent) {
        if let Err(err) = self.events_tx.send(HostEventFrame::Event { event }) {
            warn!("Failed to send event {err:?}");
        }
    }
}

pub(crate) struct HostEventTask {
    pub events: HostEventSender,
    _task: Task,
}

impl HostEventTask {
    fn new(
        session_id: uuid::Uuid,
        session: actix_ws::Session,
        msg_stream: actix_ws::MessageStream,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        let (events_tx, events_rx) = mpsc::unbounded_channel();

        let _task = Task::spawn_local(Self::task_entry(
            session_id,
            session,
            msg_stream,
            events_tx.clone(),
            events_rx,
            statemachine_proxy,
        ));

        let events = HostEventSender { events_tx };
        Self { events, _task }
    }

    pub(crate) fn send(&self, event: HostEvent) {
        self.events.send(event);
    }

    async fn task_entry(
        session_id: uuid::Uuid,
        session: actix_ws::Session,
        msg_stream: actix_ws::MessageStream,
        events_tx: mpsc::UnboundedSender<HostEventFrame>,
        events_rx: mpsc::UnboundedReceiver<HostEventFrame>,
        statemachine_proxy: StateMachineProxy,
    ) {
        let (ping_task_tx, ping_task_rx) = mpsc::channel(32);

        let mut ping_task = PingTask {
            session_id,
            ping_rx: ping_task_rx,
            events_tx,
            statemachine_proxy: statemachine_proxy.clone(),
        };

        tokio::select! {
            _ = Self::process_rx_events(session_id, msg_stream, ping_task_tx, statemachine_proxy.clone()) => {}
            _ = Self::process_tx_events(session_id, session.clone(), events_rx, statemachine_proxy) => {}
            _ = ping_task.run() => {}
        }

        let _ = session.close(None).await;
    }

    async fn process_rx_events(
        session_id: uuid::Uuid,
        mut stream: actix_ws::MessageStream,
        ping_task_tx: mpsc::Sender<u64>,
        statemachine_proxy: StateMachineProxy,
    ) {
        loop {
            let msg = match stream.recv().await {
                None | Some(Ok(actix_ws::Message::Close(_))) => {
                    info!("End of stream");
                    break;
                }
                Some(Ok(actix_ws::Message::Text(msg))) => msg,
                Some(Ok(_)) => {
                    continue;
                }
                Some(Err(err)) => {
                    warn!("recv() failed: {err:?}");
                    break;
                }
            };

            match serde_json::from_str(&msg) {
                Ok(HostEventFrame::Ping { id }) => {
                    warn!("Unexpected Ping({id})");
                }
                Ok(HostEventFrame::Pong { id }) => {
                    let ret = ping_task_tx.send(id).await;
                    if ret.is_err() {
                        warn!("Failed to send ping to Ping task");
                    }
                }
                Ok(HostEventFrame::Event { event }) => {
                    info!("Got event: {event:?}");
                }
                Err(err) => {
                    error!("Fail to parse {msg}: {err:?}");
                }
            }
        }

        statemachine_proxy
            .send(StateMachineEvent::HostEvents {
                session_id,
                notification: Notification::ConnectionLost,
            })
            .await;
    }

    async fn process_tx_events(
        session_id: uuid::Uuid,
        mut session: actix_ws::Session,
        mut events_rx: mpsc::UnboundedReceiver<HostEventFrame>,
        statemachine_proxy: StateMachineProxy,
    ) {
        while let Some(msg) = events_rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(json) => json,
                Err(err) => {
                    warn!("Fail to convert message to JSON {msg:?}: {err:?}");
                    continue;
                }
            };

            let ret = session.text(json).await;
            if let Err(err) = ret {
                warn!("Websocket send failed: {err:?}");

                statemachine_proxy
                    .send(StateMachineEvent::HostEvents {
                        session_id,
                        notification: Notification::ConnectionLost,
                    })
                    .await;

                break;
            }
        }
    }
}

/// Waits for and validates the first WebSocket message containing authentication credentials.
///
/// This implements the "credentials as first message" pattern for WebSocket authentication,
/// chosen over query parameters to avoid token leakage in server logs.
///
/// ## Authentication Flow
///
/// 1. Client establishes WebSocket connection (no auth yet)
/// 2. Server accepts connection but doesn't authorize
/// 3. This function waits for the first message (5-second timeout)
/// 4. Client sends JSON: `{"token": "<one-time-token>"}`
/// 5. Server extracts token and returns it for validation
/// 6. Caller validates token against session
/// 7. If valid → `pending_sender.accept()` sends "Welcome" and activates connection
/// 8. If invalid → `pending_sender.reject()` closes connection with error
///
/// ## Security Design
///
/// - **One-time tokens**: Tokens are generated per login and cannot be reused
/// - **First-message only**: No other messages are accepted before authentication
/// - **Timeout**: 5-second window prevents connection hanging
/// - **No URL exposure**: Token never appears in WebSocket URL or server logs
///
/// ## Message Format
///
/// Expected first message:
/// ```json
/// {"token": "7vaag596mfrsvlzzo8298yhv70k7hrrw..."}
/// ```
///
/// ## Returns
///
/// - `Ok((token, pending_sender))`: Authentication message received successfully
/// - `Err(WsAuthTimeout)`: No message received within 5 seconds
/// - `Err(ChannelClosedRx)`: Connection closed before authentication
/// - `Err(WsInvalidAuthMessage)`: Message format invalid or not a text message
pub(crate) async fn wait_authentication(
    session: actix_ws::Session,
    msg_stream: actix_ws::MessageStream,
) -> Result<(String, PendingHostEventSender)> {
    let mut pending_sender = PendingHostEventSender {
        session,
        msg_stream,
    };

    let Ok(resp) =
        tokio::time::timeout(Duration::from_secs(5), pending_sender.msg_stream.recv()).await
    else {
        pending_sender.reject().await;
        return Err(Error::WsAuthTimeout);
    };

    let msg = match resp {
        None | Some(Ok(actix_ws::Message::Close(_))) => {
            pending_sender.reject().await;
            return Err(Error::ChannelClosedRx);
        }
        Some(Ok(actix_ws::Message::Text(msg))) => msg,
        _ => {
            pending_sender.reject().await;
            return Err(Error::WsInvalidAuthMessage);
        }
    };

    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let Ok(token_holder) = serde_json::from_str::<Token>(&msg) else {
        pending_sender.reject().await;
        return Err(Error::WsInvalidAuthMessage);
    };

    Ok((token_holder.token, pending_sender))
}

/// A WebSocket connection in the authentication pending state.
///
/// This represents a WebSocket connection that has been accepted at the transport
/// level but not yet authorized. It can transition to either:
/// - `HostEventSender` via `accept()` - Authentication succeeded
/// - Closed connection via `reject()` - Authentication failed
///
/// This two-phase approach (accept transport, then authenticate) allows the server
/// to receive and validate credentials before fully activating the connection.
pub(crate) struct PendingHostEventSender {
    session: actix_ws::Session,
    msg_stream: actix_ws::MessageStream,
}

impl PendingHostEventSender {
    /// Accepts the WebSocket connection after successful authentication.
    ///
    /// Sends a "Welcome" confirmation message to the client and promotes the
    /// pending connection to a fully authorized `HostEventSender`.
    ///
    /// # Parameters
    /// - `session_id`: UUID of the session this connection belongs to
    /// - `proxy`: State machine proxy for coordinating streaming services
    ///
    /// # Returns
    /// - `Ok(HostEventSender)`: Connection is now fully authorized and active
    /// - `Err(ChannelClosedTx)`: Failed to send welcome message (connection died)
    pub(crate) async fn accept(
        mut self,
        session_id: uuid::Uuid,
        proxy: StateMachineProxy,
    ) -> Result<HostEventTask> {
        const SUCCESS_JSON: &str = "\"Welcome\"";
        let Ok(_) = self.session.text(SUCCESS_JSON).await else {
            self.reject().await;
            return Err(Error::ChannelClosedTx);
        };

        Ok(HostEventTask::new(
            session_id,
            self.session,
            self.msg_stream,
            proxy,
        ))
    }

    /// Rejects the WebSocket connection due to failed authentication.
    ///
    /// Closes the connection with a Protocol error code and "Invalid credentials"
    /// message. This clearly indicates to the client that authentication failed.
    pub(crate) async fn reject(self) {
        let reason = CloseReason {
            code: CloseCode::Protocol,
            description: Some("Invalid credentials".to_owned()),
        };
        let _ = self.session.close(Some(reason)).await;
    }
}
