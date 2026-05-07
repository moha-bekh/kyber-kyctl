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

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use async_trait::async_trait;
use log::{error, info, warn};
use tokio::sync::oneshot;
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, MessageEvent};

use crate::host_events::{self, HostEventFrame, HostEventSink};
use crate::platform;
use crate::platform::{WebSocket, WebSocketHandler};
use crate::{
    ConnectionParams, ControlPlaneCapabilities, ControlPlaneCloseReason, Error, Message, PeerInfo,
    Result,
};

use super::dom;

#[derive(Debug)]
enum ConnectionResult {
    Ok,
    Failed,
}

struct Timer {
    id: i32,
    _closure: Closure<dyn FnMut()>,
}

enum ConnectionState {
    Connecting {
        timer: Option<Timer>,
        token: String,
        connected_tx: oneshot::Sender<ConnectionResult>,
    },
    Authentified {
        timer: Option<Timer>,
    },
    Closed,
}

struct Inner {
    ws: Rc<WebSocket>,
    timer_duration: Duration,
    event_sink: Rc<HostEventSink>,
    state: RefCell<ConnectionState>,
}

impl Inner {
    async fn new(
        url: String,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
        auth_token: &str,
    ) -> Result<Rc<Self>> {
        // Connect
        let ws = WebSocket::new(&url)?;

        let timer_duration = host_events::timeout_duration(capabilities);
        info!("Setting Control Plane timeout duration to {timer_duration:?}");

        let (connected_tx, connected_rx) = oneshot::channel();
        let state = ConnectionState::Connecting {
            timer: None,
            token: auth_token.to_owned(),
            connected_tx,
        };

        let this = Rc::new(Self {
            ws: ws.clone(),
            timer_duration,
            event_sink: Rc::new(event_sink),
            state: RefCell::new(state),
        });

        ws.plug_handler(this.clone())?;

        match connected_rx.await {
            Ok(ConnectionResult::Ok) => Ok(this),
            Ok(ConnectionResult::Failed) => Err(Error::WsConnectFailed),
            Err(_) => Err(Error::WsConnectFailed),
        }
    }

    fn stop(&self) -> Result<()> {
        self.clear_timeout(&mut self.state.borrow_mut())?;
        self.ws.unplug_handler()?;

        Ok(())
    }

    fn clear_timeout(&self, state: &mut ConnectionState) -> Result<()> {
        let timer = match *state {
            ConnectionState::Connecting { ref mut timer, .. } => timer,
            ConnectionState::Authentified { ref mut timer, .. } => timer,
            _ => return Ok(()),
        };

        if let Some(timer) = timer.take() {
            let window = dom::window()?;
            window.clear_timeout_with_handle(timer.id);
        }

        Ok(())
    }

    fn arm_timeout(&self, duration: Duration, state: &mut ConnectionState) -> Result<()> {
        let event_sink = self.event_sink.clone();

        let ontimeout_cb = Closure::<dyn FnMut()>::new(move || {
            event_sink.blocking_push_msg(Message::ControlPlaneClosed {
                reason: ControlPlaneCloseReason::Timeout,
            });
        });

        let window = dom::window()?;
        let new_timer_id = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            ontimeout_cb.as_ref().unchecked_ref(),
            duration.as_millis() as _,
        )?;

        let timer = match state {
            ConnectionState::Connecting { ref mut timer, .. } => timer,
            ConnectionState::Authentified { ref mut timer, .. } => timer,
            _ => return Ok(()),
        };

        if let Some(timer) = timer.take() {
            window.clear_timeout_with_handle(timer.id);
        }

        *timer = Some(Timer {
            id: new_timer_id,
            _closure: ontimeout_cb,
        });

        Ok(())
    }

    fn handle_authentication(&self, msg: String) -> bool {
        let Ok(status) = serde_json::from_str::<&str>(&msg) else {
            return false;
        };

        status == "Welcome"
    }

    fn handle_authentified_messages(&self, msg: String, state: &mut ConnectionState) -> Result<()> {
        match serde_json::from_str(&msg) {
            Ok(HostEventFrame::Event { event }) => {
                self.event_sink.blocking_push(event);
            }
            Ok(HostEventFrame::Ping { id }) => {
                let pong = HostEventFrame::Pong { id };
                let pong = match serde_json::to_string(&pong) {
                    Ok(pong) => pong,
                    Err(err) => {
                        warn!("Fail to convert pong to JSON {pong:?}: {err:?}");
                        return Ok(());
                    }
                };

                self.arm_timeout(self.timer_duration, state)?;

                let ret = self.ws.ws.send_with_str(&pong);
                if let Err(err) = ret {
                    warn!("Failed to send Pong: {err:?}");
                }
            }
            Ok(HostEventFrame::Pong { id }) => {
                warn!("Unexpected Pong({id})");
            }
            Err(err) => {
                error!("Fail to parse {msg}: {err:?}");
            }
        }

        Ok(())
    }

    fn send_token(&self) -> Result<()> {
        let mut state = self.state.borrow_mut();
        let ConnectionState::Connecting { ref token, .. } = *state else {
            return Ok(());
        };
        let token = format!("{{\"token\":\"{token}\"}}");
        self.arm_timeout(self.timer_duration, &mut state)?;
        Ok(self.ws.ws.send_with_str(&token)?)
    }
}

impl WebSocketHandler for Inner {
    const NAME: &'static str = "Control";

    // Event handlers
    fn on_open(&self) -> Result<()> {
        if self.send_token().is_err() {
            if let ConnectionState::Connecting { connected_tx, .. } =
                self.state.replace(ConnectionState::Closed)
            {
                let _ = connected_tx.send(ConnectionResult::Failed);
            }
        }
        Ok(())
    }

    fn on_message(&self, msg: MessageEvent) -> Result<()> {
        // Unpack MessageEvent
        let Ok(msg) = msg.data().dyn_into::<js_sys::JsString>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        let msg: String = msg.into();
        {
            let mut state = self.state.borrow_mut();
            match *state {
                ConnectionState::Authentified { .. } => {
                    return self.handle_authentified_messages(msg, &mut state)
                }
                ConnectionState::Closed => return Ok(()),
                _ => (), // Connecting handled after as it changes the state.
            }
        }

        // Handling ConnectionState::Connecting

        // TODO: Overall this code begs for a proper type state pattern refactor on the Inner
        // connection but the internal mutability makes it challenging.
        let (new_state, result) = if self.handle_authentication(msg) {
            (
                ConnectionState::Authentified { timer: None },
                ConnectionResult::Ok,
            )
        } else {
            (ConnectionState::Closed, ConnectionResult::Failed)
        };

        let mut old_state = self.state.replace(new_state);
        self.clear_timeout(&mut old_state)?;
        if let ConnectionState::Connecting { connected_tx, .. } = old_state {
            let _ = connected_tx.send(result);
        }
        Ok(())
    }

    fn on_close(&self, event: CloseEvent) -> Result<()> {
        let mut state = self.state.replace(ConnectionState::Closed);

        self.clear_timeout(&mut state)?;
        match state {
            ConnectionState::Connecting { connected_tx, .. } => {
                let _ = connected_tx.send(ConnectionResult::Failed);
            }
            ConnectionState::Authentified { .. } => {
                self.event_sink
                    .blocking_push_msg(Message::ControlPlaneClosed {
                        reason: ControlPlaneCloseReason::Closed {
                            was_clean: event.was_clean(),
                            code: Some(event.code()),
                            reason: Some(event.reason()),
                        },
                    });
            }
            ConnectionState::Closed => (),
        }

        Ok(())
    }
}

/// HACK: Inner requires to be Send on functions that will .await after
/// calling `Inner::new()`.
///
/// Inner contains a web_sys::Websocket that isn't Send. Furthermore, the Rc<>
/// isn't send too.
///
/// Since we are in a browser (single-threaded environment) and we don't want
/// to send the value to a worker, it is acceptable to force Rc<Inner> to be
/// Send.
struct InnerRef(Rc<Inner>);

pub(crate) struct WsClient {
    _inner: InnerRef,
}

impl WsClient {
    async fn new(
        conn_params: &ConnectionParams,
        _peer_info: &PeerInfo,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
        auth_token: &str,
    ) -> Result<Self> {
        let host = platform::http::create_host(conn_params);
        let url = format!("wss://{host}/ws");

        let inner = InnerRef(Inner::new(url, capabilities, event_sink, auth_token).await?);

        Ok(Self { _inner: inner })
    }
}

#[async_trait(?Send)]
impl crate::host_events::WsClient for WsClient {
    async fn stop(self) -> Result<()> {
        let ret = self._inner.0.stop();
        if let Err(err) = ret {
            warn!("Failed to stop HostEvent::Inner: {err:?}");
        }

        Ok(())
    }
}

pub(crate) struct WsClientBuilder;

#[async_trait(?Send)]
impl crate::host_events::WsClientBuilder for WsClientBuilder {
    type Type = WsClient;

    async fn new(
        conn_params: &ConnectionParams,
        peer_info: &PeerInfo,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
        auth_token: &str,
    ) -> Result<Self::Type> {
        WsClient::new(conn_params, peer_info, capabilities, event_sink, auth_token).await
    }
}
