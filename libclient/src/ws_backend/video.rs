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
