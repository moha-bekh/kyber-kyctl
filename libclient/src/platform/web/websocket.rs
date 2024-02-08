use std::rc::Rc;

use async_trait::async_trait;
use log::{error, info};
use wasm_bindgen::prelude::*;
use web_sys::{BinaryType, CloseEvent, MessageEvent};

use crate::Result;

#[async_trait(?Send)]
pub(crate) trait WebSocketHandler {
    const NAME: &'static str;

    fn on_open(self: &Rc<Self>) -> Result<()> {
        Ok(())
    }

    fn on_close(self: &Rc<Self>, _event: CloseEvent) -> Result<()> {
        Ok(())
    }

    fn on_message(self: &Rc<Self>, event: MessageEvent) -> Result<()>;
}

pub(crate) struct WebSocket {
    pub(crate) ws: web_sys::WebSocket,
}

impl WebSocket {
    pub(crate) fn new(url: &str) -> Result<Rc<Self>> {
        let ws = web_sys::WebSocket::new(url)?;
        ws.set_binary_type(BinaryType::Arraybuffer);

        Ok(Rc::new(Self { ws }))
    }

    pub(crate) fn plug_handler<T>(&self, handler: Rc<T>) -> Result<()>
    where
        T: WebSocketHandler + 'static,
    {
        // onopen
        let handler_clone = handler.clone();
        let onopen_cb = Closure::<dyn FnMut()>::new(move || {
            info!("Websocket '{}' opened", T::NAME);

            let ret = handler_clone.on_open();
            if let Err(err) = ret {
                error!("Websocket {} onopen failed: {err:?}", T::NAME);
            }
        });
        self.ws.set_onopen(Some(onopen_cb.as_ref().unchecked_ref()));
        onopen_cb.forget();

        // onclose
        let handler_clone = handler.clone();
        let onclose_cb = Closure::<dyn FnMut(_)>::new(move |event: CloseEvent| {
            info!(
                "Websocket '{}' closed. code: {code} - reason: '{reason}' - wasClean: {wasclean}",
                T::NAME,
                code = event.code(),
                reason = event.reason(),
                wasclean = event.was_clean()
            );

            let ret = handler_clone.on_close(event);
            if let Err(err) = ret {
                error!("Websocket {} onclose failed: {err:?}", T::NAME);
            }
        });
        self.ws
            .set_onclose(Some(onclose_cb.as_ref().unchecked_ref()));
        onclose_cb.forget();

        // onmessage
        let handler_clone = handler.clone();
        let onmessage_cb = Closure::<dyn FnMut(_)>::new(move |event| {
            let ret = handler_clone.on_message(event);
            if let Err(err) = ret {
                error!("Websocket {} onmessage failed: {err:?}", T::NAME);
            }
        });
        self.ws
            .set_onmessage(Some(onmessage_cb.as_ref().unchecked_ref()));
        onmessage_cb.forget();

        Ok(())
    }
}
