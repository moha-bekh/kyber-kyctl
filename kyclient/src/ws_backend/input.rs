// Project Kyber: input.rs
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
use std::sync::Arc;

use log::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{CloseEvent, MessageEvent};

use kynput::bufferproxy::{BufferProxyReceiver, BufferProxySender};
use kynput::{InputNetworkStreamMsg, InputNetworkStreamObserver};

use crate::platform::{WebSocket, WebSocketHandler};
use crate::Result;

pub struct InputWebSocketHandler {
    ws: Rc<WebSocket>,
    msg_sink: Arc<dyn InputNetworkStreamObserver>,
    proxy_sender: BufferProxySender,
    proxy_receiver: RefCell<Option<BufferProxyReceiver>>,
}

impl InputWebSocketHandler {
    pub fn new(
        url: &str,
        proxy_sender: BufferProxySender,
        proxy_receiver: BufferProxyReceiver,
        msg_sink: Arc<dyn InputNetworkStreamObserver>,
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

    pub fn stop(&self) -> Result<()> {
        self.ws.unplug_handler()?;

        Ok(())
    }

    // Tasks
    async fn tx_task(ws: Rc<WebSocket>, mut rx: BufferProxyReceiver) -> Result<()> {
        while let Ok(buf) = rx.recv().await {
            ws.ws.send_with_u8_array(&buf)?;
        }

        Ok(())
    }
}

impl WebSocketHandler for InputWebSocketHandler {
    const NAME: &'static str = "Input";

    // Event handlers
    fn on_open(&self) -> Result<()> {
        self.msg_sink
            .on_message(InputNetworkStreamMsg::ClientConnected);

        // Spawn Pipeline => WS task
        if let Some(proxy_receiver) = self.proxy_receiver.borrow_mut().take() {
            let ws = self.ws.clone();
            spawn_local(async move {
                let ret = Self::tx_task(ws, proxy_receiver).await;
                if let Err(err) = ret {
                    warn!("Websocket input tx task ended with error {err:?}");
                }
            });
        } else {
            warn!("None ProxyReceiver. on_open() call more than once");
        }

        Ok(())
    }

    fn on_close(&self, _event: CloseEvent) -> Result<()> {
        self.msg_sink
            .on_message(InputNetworkStreamMsg::ClientDisconnected);

        Ok(())
    }

    fn on_message(&self, msg: MessageEvent) -> Result<()> {
        let Ok(buf) = msg.data().dyn_into::<js_sys::ArrayBuffer>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        let buf = js_sys::Uint8Array::new(&buf).to_vec();

        self.proxy_sender.blocking_send(buf)?;

        Ok(())
    }
}
