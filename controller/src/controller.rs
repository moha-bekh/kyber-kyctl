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

        if let Some(mut avservice) = self.avservice_video.take() {
            avservice.stop().await?;
        }

        if let Some(mut avservice) = self.avservice_audio.take() {
            avservice.stop().await?;
        }

        if let Some(mut inputservice) = self.inputservice.take() {
            inputservice.stop().await?;
        }

        Ok(())
    }
}
