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

    pub(crate) displays: Vec<Display>,

    pub(crate) config: config::Config,
    pub(crate) tls_config: tls::Config,
    pub(crate) kymux_server: Option<KymuxServer>,
    pub(crate) kymux_connection: Option<kymux::Connection>,

    pub(crate) ws_router: Option<Arc<ws::WsRouter>>,

    pub(crate) avservice_video: Option<avservice::Handler>,
    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) inputservice: Option<inputservice::Handler>,

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
            avservice_video: None,
            avservice_audio: None,
            inputservice: None,
            host_event_sender: Default::default(),
            state_machine_proxy: None,
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stop");

        stop_avservice(self.avservice_video.take()).await?;
        stop_avservice(self.avservice_audio.take()).await?;
        stop_input(self.inputservice.take()).await?;
        stop_host_event_sender(self.host_event_sender.take()).await?;
        stop_wsrouter(self.ws_router.take()).await?;
        stop_kymux(self.kymux_server.take(), self.kymux_connection.take()).await?;

        Ok(())
    }

    pub(crate) fn display_known(&self, display_id: u32) -> bool {
        self.displays.iter().any(|d| d.id == display_id)
    }
}
