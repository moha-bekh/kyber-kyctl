use std::sync::Arc;

use log::{info, warn};

use avservice_types::Display;
use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::config;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::kymux_controller::KymuxBackend;
use crate::tls;
use crate::ws::WebsocketBackend;
use crate::HostEventSender;
use crate::StateMachineProxy;

pub(crate) enum DataPlaneBackend {
    Rtp,
    Kymux(KymuxBackend),
    Websocket(WebsocketBackend),
}

pub(crate) struct ClientSession {
    pub(crate) host_event_sender: HostEventSender,
    pub(crate) data_plane: Option<DataPlaneBackend>,
}

impl ClientSession {
    pub(crate) fn new(host_event_sender: HostEventSender) -> Self {
        Self {
            host_event_sender,
            data_plane: None,
        }
    }

    pub(crate) async fn stop_dataplane(&mut self) -> Result<()> {
        match self.data_plane.take() {
            Some(DataPlaneBackend::Rtp) => {}
            Some(DataPlaneBackend::Kymux(backend)) => {
                backend.stop().await?;
            }
            Some(DataPlaneBackend::Websocket(backend)) => {
                backend.stop().await?;
            }
            None => {}
        }

        Ok(())
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        info!("Stop dataplane");
        self.stop_dataplane().await?;

        info!("Stop running HostEventSender");
        self.host_event_sender.stop().await?;

        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct StreamingServices {
    pub(crate) video: Option<avservice::Handler>,
    pub(crate) audio: Option<avservice::Handler>,
    pub(crate) input: Option<inputservice::Handler>,
}

impl StreamingServices {
    async fn stop_avservice(handler: Option<avservice::Handler>) -> Result<()> {
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

    async fn stop_input(handler: Option<inputservice::Handler>) -> Result<()> {
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

    async fn stop(&mut self) -> Result<()> {
        Self::stop_avservice(self.video.take()).await?;
        Self::stop_avservice(self.audio.take()).await?;
        Self::stop_input(self.input.take()).await?;

        Ok(())
    }
}

pub struct Controller {
    pub(crate) process_factory: Arc<ProcessFactory>,

    pub(crate) displays: Vec<Display>,

    pub(crate) config: config::Config,
    pub(crate) tls_config: tls::Config,

    pub(crate) client_session: Option<ClientSession>,
    pub(crate) services: StreamingServices,

    pub(crate) state_machine_proxy: Option<StateMachineProxy>,
}

impl Controller {
    pub(crate) fn new(
        process_factory: Arc<ProcessFactory>,
        config: config::Config,
        tls_config: tls::Config,
    ) -> Self {
        Self {
            process_factory,
            displays: vec![],
            config,
            tls_config,
            client_session: None,
            services: Default::default(),
            state_machine_proxy: None,
        }
    }

    pub async fn stop_streaming(&mut self) -> Result<()> {
        info!("Stop streaming");

        if let Some(client_session) = self.client_session.as_mut() {
            let ret = client_session.stop_dataplane().await;
            if let Err(err) = ret {
                warn!("Failed to stop client session: {err:?}");
            }
        }

        let ret = self.services.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop streaming services: {err:?}");
        }

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stop");

        if let Some(client_session) = self.client_session.take() {
            let ret = client_session.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop client session: {err:?}");
            }
        }

        let ret = self.services.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop streaming services: {err:?}");
        }

        Ok(())
    }

    pub(crate) fn has_data_plane(&self) -> Result<bool> {
        let Some(client_session) = self.client_session.as_ref() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        Ok(client_session.data_plane.is_some())
    }

    pub(crate) fn display_known(&self, display_id: u32) -> bool {
        self.displays.iter().any(|d| d.id == display_id)
    }
}
