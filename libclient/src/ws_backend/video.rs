use std::rc::Rc;

use log::info;
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;

use crate::platform::player::{FrameSink, VideoPlayer};
use crate::platform::{WebSocket, WebSocketHandler};
use crate::Result;

pub struct VideoWsSocketHandler {
    ws: Rc<WebSocket>,
    player: Rc<VideoPlayer>,
}

impl VideoWsSocketHandler {
    pub fn new(url: &str, player: Rc<VideoPlayer>) -> Result<Rc<Self>> {
        let ws = WebSocket::new(url)?;

        let this = Rc::new(Self {
            ws: ws.clone(),
            player,
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
