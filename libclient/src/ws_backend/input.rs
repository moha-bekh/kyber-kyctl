use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use log::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{CloseEvent, MessageEvent};

use libkynput::bufferproxy::{BufferProxyReceiver, BufferProxySender};
use libkynput::{InputNetworkStreamMsg, InputNetworkStreamObserver};

use crate::platform::{WebSocket, WebSocketHandler};
use crate::Result;

pub struct InputWebSocketHandler {
    ws: Rc<WebSocket>,
    msg_sink: Arc<dyn InputNetworkStreamObserver + Send + Sync>,
    proxy_sender: BufferProxySender,
    proxy_receiver: RefCell<Option<BufferProxyReceiver>>,
}

impl InputWebSocketHandler {
    pub fn new(
        url: &str,
        proxy_sender: BufferProxySender,
        proxy_receiver: BufferProxyReceiver,
        msg_sink: Arc<dyn InputNetworkStreamObserver + Send + Sync>,
    ) -> Result<Rc<Self>> {
        // Connect
        let ws = WebSocket::new(url)?;

        let this = Rc::new(Self {
            ws: ws.clone(),
            msg_sink,
            proxy_sender,
            proxy_receiver: RefCell::new(Some(proxy_receiver)),
        });

        ws.plug_handler(this.clone())?;

        Ok(this)
    }

    // Tasks
    async fn tx_task(self: Rc<Self>, mut rx: BufferProxyReceiver) -> Result<()> {
        while let Ok(buf) = rx.recv().await {
            self.ws.ws.send_with_u8_array(&buf)?;
        }

        Ok(())
    }
}

impl WebSocketHandler for InputWebSocketHandler {
    const NAME: &'static str = "Input";

    // Event handlers
    fn on_open(self: &Rc<Self>) -> Result<()> {
        self.msg_sink
            .on_message(InputNetworkStreamMsg::ClientConnected);

        // Spawn Pipeline => WS task
        if let Some(proxy_receiver) = self.proxy_receiver.borrow_mut().take() {
            let this = self.clone();
            spawn_local(async move {
                let ret = this.tx_task(proxy_receiver).await;
                if let Err(err) = ret {
                    warn!("Websocket input tx task ended with error {err:?}");
                }
            });
        } else {
            warn!("None ProxyReceiver. on_open() call more than once");
        }

        Ok(())
    }

    fn on_close(self: &Rc<Self>, _event: CloseEvent) -> Result<()> {
        self.msg_sink
            .on_message(InputNetworkStreamMsg::ClientDisconnected);

        Ok(())
    }

    fn on_message(self: &Rc<Self>, msg: MessageEvent) -> Result<()> {
        let Ok(buf) = msg.data().dyn_into::<js_sys::ArrayBuffer>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        let buf = js_sys::Uint8Array::new(&buf).to_vec();

        self.proxy_sender.blocking_send(buf)?;

        Ok(())
    }
}
