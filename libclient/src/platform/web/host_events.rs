use std::rc::Rc;

use async_trait::async_trait;
use log::info;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

use crate::host_events::HostEventSink;
use crate::platform::{WebSocket, WebSocketHandler};
use crate::{ConnectionParams, Result};

struct Inner {
    _ws: WebSocket,
    event_sink: HostEventSink,
}

impl Inner {
    fn new(url: String, event_sink: HostEventSink) -> Result<Rc<Self>> {
        // Connect
        let ws = WebSocket::new(&url)?;

        let this = Rc::new(Self {
            _ws: ws.clone(),
            event_sink,
        });

        ws.plug_handler(this.clone())?;

        Ok(this)
    }
}

impl WebSocketHandler for Inner {
    const NAME: &'static str = "Control";

    // Event handlers
    fn on_message(self: &Rc<Self>, msg: MessageEvent) -> Result<()> {
        // Unpack MessageEvent
        let Ok(msg) = msg.data().dyn_into::<js_sys::JsString>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        let msg: String = msg.into();
        self.event_sink.blocking_push(&msg);

        Ok(())
    }
}

pub(crate) struct WsClient {
    _inner: Rc<Inner>,
}

impl WsClient {
    async fn new(conn_params: &ConnectionParams, event_sink: HostEventSink) -> Result<Self> {
        let url = format!(
            "wss://{host}:{port}/ws",
            host = conn_params.host,
            port = conn_params.port
        );

        let inner = Inner::new(url, event_sink)?;

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

    async fn new(conn_params: &ConnectionParams, event_sink: HostEventSink) -> Result<Self::Type> {
        WsClient::new(conn_params, event_sink).await
    }
}
