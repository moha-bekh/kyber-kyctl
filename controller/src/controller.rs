use log::info;

use libkypc::process::ProcessFactory;

use crate::error::Result;
use crate::inputservice;
use crate::streamer;

pub struct Controller {
    pub(crate) process_factory: ProcessFactory,

    pub(crate) streamer_video: Option<streamer::Handler>,
    pub(crate) streamer_audio: Option<streamer::Handler>,
    pub(crate) inputservice: Option<inputservice::Handler>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            process_factory: ProcessFactory::new(false),
            streamer_video: None,
            streamer_audio: None,
            inputservice: None,
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

        match self.inputservice.as_mut() {
            Some(inputservice) => {
                inputservice.stop().await?;
                self.inputservice = None
            }
            None => {
                info!("No active Input Server");
            }
        }

        Ok(())
    }
}
