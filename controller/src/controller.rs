use log::info;

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::error::Result;
use crate::inputservice;

pub(crate) async fn stop_avservice(handler: Option<avservice::Handler>) -> Result<()> {
    let Some(mut handler) = handler else {
        return Ok(());
    };

    if !handler.is_running() {
        return Ok(());
    }

    info!("Stop running {} AVServer", handler.name());
    handler.stop().await?;

    Ok(())
}

pub(crate) async fn stop_input(handler: Option<inputservice::Handler>) -> Result<()> {
    let Some(mut handler) = handler else {
        return Ok(());
    };

    if !handler.is_running() {
        return Ok(());
    }

    info!("Stop running InputServer");
    handler.stop().await?;

    Ok(())
}

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

        stop_avservice(self.avservice_video.take()).await?;
        stop_avservice(self.avservice_audio.take()).await?;
        stop_input(self.inputservice.take()).await?;

        Ok(())
    }
}
