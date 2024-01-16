use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use async_trait::async_trait;
use log::{error, info, warn};
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, ErrorEvent, MessageEvent};

use crate::host_events::{self, HostEventFrame, HostEventSink};
use crate::platform::{WebSocket, WebSocketHandler};
use crate::{ConnectionParams, ControlPlaneCapabilities, Message, Result};

use super::dom;

struct Inner {
    ws: WebSocket,
    timer_duration: Duration,
    event_sink: HostEventSink,
    timer_id: RefCell<Option<i32>>,
}

impl Inner {
    fn new(
        url: String,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
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
        let ontimeout_cb = Closure::<dyn FnMut()>::new(move || {});

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
        warn!("Control websocket closed: {event:?}");

        self.clear_timeout()?;

        Ok(())
    }

    fn on_error(self: &Rc<Self>, event: ErrorEvent) -> Result<()> {
        warn!("Control websocket error: {event:?}");

        self.clear_timeout()?;

        Ok(())
    }
}

pub(crate) struct WsClient {
    _inner: Rc<Inner>,
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

        let inner = Inner::new(url, capabilities, event_sink)?;

        Ok(Self { _inner: inner })
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
