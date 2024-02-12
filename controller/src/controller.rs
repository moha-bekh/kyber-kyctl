use std::sync::Arc;

use log::{info, warn};

use avservice_types::Display;
use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::config;
use crate::error::Result;
use crate::inputservice;
use crate::kymux_controller::KymuxServer;
use crate::tls;
use crate::ws;
use crate::HostEventSender;
use crate::StateMachineProxy;

pub(crate) async fn stop_kymux(
    server: Option<KymuxServer>,
    connection: Option<kymux::Connection>,
) -> Result<()> {
    if let Some(server) = server {
        info!("Stop running Kymux Server");

        let ret = server.stop().await;
        if let Err(err) = ret {
            warn!("Kymux Server stop raised an error: {err:?}");
        }
    };

    if let Some(connection) = connection {
        info!("Stop running Kymux Connection");
        connection.stop().await?;
    };

    Ok(())
}

pub(crate) async fn stop_wsrouter(ws_router: Option<Arc<ws::WsRouter>>) -> Result<()> {
    if let Some(ws_router) = ws_router {
        ws_router.stop().await?;
    }

    Ok(())
}

pub(crate) async fn stop_host_event_sender(sender: Option<HostEventSender>) -> Result<()> {
    if let Some(sender) = sender {
        info!("Stop running HostEventSender");
        sender.stop().await?;
    };

    Ok(())
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
    pub(crate) kymux_server: Option<KymuxServer>,
    pub(crate) kymux_connection: Option<kymux::Connection>,

    pub(crate) ws_router: Option<Arc<ws::WsRouter>>,

    pub(crate) services: StreamingServices,

    pub(crate) host_event_sender: Option<HostEventSender>,
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
            kymux_server: None,
            kymux_connection: None,
            ws_router: None,
            services: Default::default(),
            host_event_sender: Default::default(),
            state_machine_proxy: None,
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stop");

        let ret = self.services.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop streaming services: {err:?}");
        }

        stop_host_event_sender(self.host_event_sender.take()).await?;
        stop_wsrouter(self.ws_router.take()).await?;
        stop_kymux(self.kymux_server.take(), self.kymux_connection.take()).await?;

        Ok(())
    }

    pub(crate) fn display_known(&self, display_id: u32) -> bool {
        self.displays.iter().any(|d| d.id == display_id)
    }
}
