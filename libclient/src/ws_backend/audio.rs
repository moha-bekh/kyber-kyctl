use std::rc::Rc;

use log::info;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

use crate::platform::player::{AudioPlayer, FrameSink};
use crate::platform::{WebSocket, WebSocketHandler};
use crate::Result;

pub struct AudioWsSocketHandler {
    _ws: WebSocket,
    player: Rc<AudioPlayer>,
}

impl AudioWsSocketHandler {
    pub fn new(url: &str, player: Rc<AudioPlayer>) -> Result<Rc<Self>> {
        let ws = WebSocket::new(url)?;

        let this = Rc::new(Self {
            _ws: ws.clone(),
            player,
        });

        ws.plug_handler(this.clone())?;

        Ok(this)
    }
}

impl WebSocketHandler for AudioWsSocketHandler {
    const NAME: &'static str = "Audio";

    fn on_message(self: &Rc<Self>, msg: MessageEvent) -> Result<()> {
        // Unpack MessageEvent
        let Ok(buf) = msg.data().dyn_into::<js_sys::ArrayBuffer>() else {
            info!("Unexpected message type");
            return Ok(());
        };

        self.player.send_frame(buf)?;

        Ok(())
    }
}
