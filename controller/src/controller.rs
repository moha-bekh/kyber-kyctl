use std::sync::Arc;

use log::info;

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::error::Result;
use crate::inputservice;
use crate::kymux_controller::KymuxListener;
use crate::tls;

pub(crate) async fn stop_kymux(
    listener: Option<KymuxListener>,
    connection: Option<kymux::Connection>,
) -> Result<()> {
    if let Some(listener) = listener {
        info!("Stop running Kymux Listener");
        listener.stop().await?;
    };

    if let Some(mut connection) = connection {
        info!("Stop running Kymux Connection");
        connection.stop().await?;
    };

    Ok(())
}

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
    pub(crate) process_factory: Arc<ProcessFactory>,

    pub(crate) tls_config: tls::Config,
    pub(crate) kymux_listener: Option<KymuxListener>,
    pub(crate) kymux: Option<kymux::Connection>,
    pub(crate) avservice_video: Option<avservice::Handler>,
    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) inputservice: Option<inputservice::Handler>,
}

impl Controller {
    pub(crate) fn new(process_factory: Arc<ProcessFactory>, tls_config: tls::Config) -> Self {
        Self {
            process_factory,
            tls_config,
            kymux_listener: None,
            kymux: None,
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
        stop_kymux(self.kymux_listener.take(), self.kymux.take()).await?;

        Ok(())
    }
}
