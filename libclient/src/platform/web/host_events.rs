use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use async_trait::async_trait;
use log::{error, info, warn};
use tokio::sync::oneshot;
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, MessageEvent};

use crate::host_events::{self, HostEventFrame, HostEventSink};
use crate::platform::{WebSocket, WebSocketHandler};
use crate::{
    ConnectionParams, ControlPlaneCapabilities, ControlPlaneCloseReason, Error, Message, Result,
};

use super::dom;

#[derive(Debug)]
enum ConnectionResult {
    Ok,
    Failed,
}

struct Inner {
    ws: WebSocket,
    timer_duration: Duration,
    event_sink: HostEventSink,
    timer_id: RefCell<Option<i32>>,
    started_tx: RefCell<Option<oneshot::Sender<ConnectionResult>>>,
}

impl Inner {
    fn new(
        url: String,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
        started_tx: oneshot::Sender<ConnectionResult>,
    ) -> Result<Rc<Self>> {
        // Connect
        let ws = WebSocket::new(&url)?;

        let timer_duration = host_events::timeout_duration(capabilities);
        info!("Setting Control Plane timeout duration to {timer_duration:?}");

        let this = Rc::new(Self {
            ws: ws.clone(),
            timer_duration,
            event_sink,
            timer_id: RefCell::new(None),
            started_tx: RefCell::new(Some(started_tx)),
        });

        ws.plug_handler(this.clone())?;

        Ok(this)
    }

    fn clear_timeout(self: &Rc<Self>) -> Result<()> {
        let timer_id = self.timer_id.borrow_mut();
        if let Some(timer_id) = &*timer_id {
            let window = dom::window()?;
            window.clear_timeout_with_handle(*timer_id);
        }

        Ok(())
    }

    fn arm_timeout(self: &Rc<Self>, duration: Duration) -> Result<()> {
        let this = self.clone();
        let ontimeout_cb = Closure::<dyn FnMut()>::new(move || {
            this.event_sink
                .blocking_push_msg(Message::ControlPlaneClosed {
                    reason: ControlPlaneCloseReason::Timeout,
                });
        });

        let window = dom::window()?;
        let new_timer_id = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            ontimeout_cb.as_ref().unchecked_ref(),
            duration.as_millis() as _,
        )?;
        ontimeout_cb.forget();

        let mut timer_id = self.timer_id.borrow_mut();
        if let Some(timer_id) = &*timer_id {
            window.clear_timeout_with_handle(*timer_id);
        }

        *timer_id = Some(new_timer_id);

        Ok(())
    }
}

impl WebSocketHandler for Inner {
    const NAME: &'static str = "Control";

    // Event handlers
    fn on_open(self: &Rc<Self>) -> Result<()> {
        self.arm_timeout(self.timer_duration)?;

        let Some(started_tx) = self.started_tx.borrow_mut().take() else {
            warn!("started_tx already consumed");
            return Ok(());
        };

        let ret = started_tx.send(ConnectionResult::Ok);
        if ret.is_err() {
            warn!("Fail to forward error");
        }

        Ok(())
    }

    fn on_message(self: &Rc<Self>, msg: MessageEvent) -> Result<()> {
        // Unpack MessageEvent
        let Ok(msg) = msg.data().dyn_into::<js_sys::JsString>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        let msg: String = msg.into();
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

                self.arm_timeout(self.timer_duration)?;

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

    fn on_close(self: &Rc<Self>, event: CloseEvent) -> Result<()> {
        self.clear_timeout()?;

        if let Some(started_tx) = self.started_tx.borrow_mut().take() {
            let ret = started_tx.send(ConnectionResult::Failed);
            if ret.is_err() {
                warn!("Fail to forward error");
            }
        } else {
            self.event_sink
                .blocking_push_msg(Message::ControlPlaneClosed {
                    reason: ControlPlaneCloseReason::Closed {
                        was_clean: event.was_clean(),
                        code: Some(event.code()),
                        reason: Some(event.reason()),
                    },
                });
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
unsafe impl Send for InnerRef {}

pub(crate) struct WsClient {
    _inner: InnerRef,
}

impl WsClient {
    async fn new(
        conn_params: &ConnectionParams,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
    ) -> Result<Self> {
        let url = format!(
            "wss://{host}:{port}/ws",
            host = conn_params.host,
            port = conn_params.port
        );

        let (connected_tx, connected_rx) = oneshot::channel();
        let inner = InnerRef(Inner::new(url, capabilities, event_sink, connected_tx)?);

        match connected_rx.await {
            Ok(ConnectionResult::Ok) => Ok(Self { _inner: inner }),
            Ok(ConnectionResult::Failed) => Err(Error::WsConnectFailed),
            Err(_) => Err(Error::WsConnectFailed),
        }
    }
}

#[async_trait(?Send)]
impl crate::host_events::WsClient for WsClient {
    async fn stop(self) -> Result<()> {
        Ok(())
    }
}

pub(crate) struct WsClientBuilder;

#[async_trait]
impl crate::host_events::WsClientBuilder for WsClientBuilder {
    type Type = WsClient;

    async fn new(
        conn_params: &ConnectionParams,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
    ) -> Result<Self::Type> {
        WsClient::new(conn_params, capabilities, event_sink).await
    }
}
