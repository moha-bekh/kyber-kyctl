use log::info;

use crate::error::Result;
use crate::input_server;
use crate::streamer_audio;
use crate::streamer_video;

pub struct Controller {
    pub streamer_video: Option<streamer_video::Handler>,
    pub streamer_audio: Option<streamer_audio::Handler>,
    pub input_server: Option<input_server::Handler>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            streamer_video: None,
            streamer_audio: None,
            input_server: None,
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stop");

        match self.streamer_video.as_mut() {
            Some(streamer) => {
                streamer.stop().await?;
                self.streamer_video = None
            }
            None => {
                info!("No active streamer");
            }
        }

        match self.streamer_audio.as_mut() {
            Some(streamer) => {
                streamer.stop().await?;
                self.streamer_audio = None
            }
            None => {
                info!("No active audio streamer");
            }
        }

        match self.input_server.as_mut() {
            Some(input_server) => {
                input_server.stop().await?;
                self.input_server = None
            }
            None => {
                info!("No active Input Server");
            }
        }

        Ok(())
    }
}
