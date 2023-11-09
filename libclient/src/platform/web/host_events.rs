use std::rc::Rc;

use async_trait::async_trait;
use log::info;
use wasm_bindgen::prelude::*;
use web_sys::{BinaryType, CloseEvent, ErrorEvent, MessageEvent, WebSocket};

use crate::host_events::HostEventSink;

use crate::{ConnectionParams, Result};

struct Inner {
    _ws: WebSocket,
    event_sink: HostEventSink,
}

impl Inner {
    fn new(url: String, event_sink: HostEventSink) -> Result<Rc<Self>> {
        // Connect
        let ws = WebSocket::new(&url)?;
        ws.set_binary_type(BinaryType::Arraybuffer);

        let this = Rc::new(Self {
            _ws: ws.clone(),
            event_sink,
        });

        // onopen
        let this_clone = this.clone();
        let onopen_cb = Closure::<dyn FnMut()>::new(move || {
            this_clone.on_open();
        });
        ws.set_onopen(Some(onopen_cb.as_ref().unchecked_ref()));
        onopen_cb.forget();

        // onclose
        let this_clone = this.clone();
        let onclose_cb = Closure::<dyn FnMut(_)>::new(move |event| {
            this_clone.on_close(event);
        });
        ws.set_onclose(Some(onclose_cb.as_ref().unchecked_ref()));
        onclose_cb.forget();

        // onerror
        let this_clone = this.clone();
        let onerror_cb = Closure::<dyn FnMut(_)>::new(move |event| {
            this_clone.on_error(event);
        });
        ws.set_onerror(Some(onerror_cb.as_ref().unchecked_ref()));
        onerror_cb.forget();

        // onmessage
        let this_clone = this.clone();
        let onmessage_cb = Closure::<dyn FnMut(_)>::new(move |event| {
            this_clone.on_message(event);
        });
        ws.set_onmessage(Some(onmessage_cb.as_ref().unchecked_ref()));
        onmessage_cb.forget();

        Ok(this)
    }

    // Event handlers
    fn on_open(self: &Rc<Self>) {
        info!("Control websocket opened");
    }

    fn on_close(self: &Rc<Self>, event: CloseEvent) {
        info!("Control websocket closed: {}", event.reason());
    }

    fn on_error(self: &Rc<Self>, event: ErrorEvent) {
        info!("Control websocket error: {event:?}");
    }

    fn on_message(self: &Rc<Self>, msg: MessageEvent) {
        // Unpack MessageEvent
        let Ok(msg) = msg.data().dyn_into::<js_sys::JsString>() else {
            info!("Unexpected message type");
            return;
        };

        let msg: String = msg.into();
        self.event_sink.blocking_push(&msg);
    }
}

pub(crate) struct WsClient {
    _inner: Rc<Inner>,
}

impl WsClient {
    async fn new(url: String, event_sink: HostEventSink) -> Result<Self> {
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

    async fn new(
        url: String,
        _conn_params: &ConnectionParams,
        event_sink: HostEventSink,
    ) -> Result<Self::Type> {
        WsClient::new(url, event_sink).await
    }
}
