// Project Kyber: video.rs
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

use log::info;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

use crate::platform::player::{PacketSink, VideoPlayer};
use crate::platform::{WebSocket, WebSocketHandler};
use crate::Result;

use super::packet::buffer_to_packet;

pub struct VideoWsSocketHandler {
    ws: Rc<WebSocket>,
    player: RefCell<VideoPlayer>,
}

impl VideoWsSocketHandler {
    pub fn new(url: &str, player: VideoPlayer) -> Result<Rc<Self>> {
        let ws = WebSocket::new(url)?;

        let this = Rc::new(Self {
            ws: ws.clone(),
            player: RefCell::new(player),
        });

        ws.plug_handler(this.clone())?;

        Ok(this)
    }

    pub fn stop(&self) -> Result<()> {
        self.ws.unplug_handler()?;

        Ok(())
    }
}

impl WebSocketHandler for VideoWsSocketHandler {
    const NAME: &'static str = "Video";

    // Event handlers
    fn on_message(&self, msg: MessageEvent) -> Result<()> {
        // Unpack MessageEvent
        let Ok(buf) = msg.data().dyn_into::<js_sys::ArrayBuffer>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        let packet = buffer_to_packet(buf)?;
        self.player.borrow_mut().send_packet(packet)?;

        Ok(())
    }
}
