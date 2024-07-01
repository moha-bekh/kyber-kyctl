use std::cell::RefCell;
use std::rc::Rc;

use log::info;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

use crate::platform::player::packet::buffer_to_packet;
use crate::platform::player::{AudioPlayer, PacketSink};
use crate::platform::{WebSocket, WebSocketHandler};
use crate::Result;

pub struct AudioWsSocketHandler {
    ws: Rc<WebSocket>,
    player: RefCell<AudioPlayer>,
}

impl AudioWsSocketHandler {
    pub fn new(url: &str, player: AudioPlayer) -> Result<Rc<Self>> {
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

impl WebSocketHandler for AudioWsSocketHandler {
    const NAME: &'static str = "Audio";

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
