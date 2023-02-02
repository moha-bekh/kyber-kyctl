use log::info;

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::error::Result;
use crate::inputservice;

pub struct Controller {
    pub(crate) process_factory: ProcessFactory,

    pub(crate) avservice_video: Option<avservice::Handler>,
    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) inputservice: Option<inputservice::Handler>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            process_factory: ProcessFactory::new(false),
            avservice_video: None,
            avservice_audio: None,
            inputservice: None,
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stop");

        match self.avservice_video.as_mut() {
            Some(avservice) => {
                avservice.stop().await?;
                self.avservice_video = None
            }
            None => {
                info!("No active Video AV service");
            }
        }

        match self.avservice_audio.as_mut() {
            Some(avservice) => {
                avservice.stop().await?;
                self.avservice_audio = None
            }
            None => {
                info!("No active Audio AV service");
            }
        }

        match self.inputservice.as_mut() {
            Some(inputservice) => {
                inputservice.stop().await?;
                self.inputservice = None
            }
            None => {
                info!("No active Input service");
            }
        }

        Ok(())
    }
}
