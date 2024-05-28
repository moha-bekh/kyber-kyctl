#[cfg_attr(feature = "desktop", path = "./desktop.rs")]
#[cfg_attr(feature = "web", path = "./web.rs")]
mod platform;

use std::sync::Arc;

use async_trait::async_trait;
use log::{error, info, warn};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{
    BackendDriver, ConnectionParams, DataPlaneCloseReason, Message, MessageProxy,
    StreamingSessionConfig,
};

pub(crate) mod host_service;

#[derive(Clone, Copy, Debug, Serialize)]
pub enum KymuxTransportBackend {
    #[cfg(feature = "desktop")]
    Quic,
    #[cfg(feature = "web")]
    Webtransport,
}

#[derive(Clone, Debug, Serialize)]
pub struct KymuxConfig {
    pub backend: KymuxTransportBackend,
    pub video_protocol: Option<kymux::VideoProtocol>,
}

async fn start_kymux(backend_config: &KymuxBackendConfig<'_>) -> Result<kymux::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(
        backend_config.http_client,
        backend_config.kymux_config.backend,
        backend_config.conn_params.port,
    )
    .await?;

    // Start Kymux connection
    let ret = match backend_config.kymux_config.backend {
        #[cfg(feature = "desktop")]
        KymuxTransportBackend::Quic => {
            info!("Connect using Quic backend");
            platform::connect_quinn(backend_config.conn_params, start_response.port).await
        }
        #[cfg(feature = "web")]
        KymuxTransportBackend::Webtransport => {
            info!("Connect using Webtransport backend");

            let Some(certificate_hash) = start_response.certificate_hash else {
                error!("No certificate hash returned");
                return Err(Error::Rejected);
            };

            platform::connect_webtransport(
                backend_config.http_client.base_url().into(),
                certificate_hash,
            )
            .await
        }
    };

    ret.map_err(|err| {
        warn!("Failed to connect to DataPlane: {err:?}");
        Error::DataPlaneConnectFailed
    })
}

async fn kymux_closed_task(
    connection: Arc<kymux::Connection>,
    msg_proxy: MessageProxy,
) -> Result<()> {
    let ret = connection.closed().await;
    info!("Kymux connection closed: {ret:?}");

    let reason = match ret {
        Ok(()) => DataPlaneCloseReason::Closed,
        Err(_) => DataPlaneCloseReason::Error,
    };

    let ret = msg_proxy
        .send_async(Message::DataPlaneClosed { reason })
        .await;
    if let Err(err) = ret {
        warn!("Fail to send DataPlaneClosed: {err:?}");
    }

    Ok(())
}

pub(crate) struct KymuxBackendConfig<'a> {
    pub(crate) conn_params: &'a ConnectionParams,
    pub(crate) http_client: &'a HttpClient,
    pub(crate) streaming_config: &'a StreamingSessionConfig,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) msg_proxy: MessageProxy,
}

pub(crate) struct KymuxBackend {
    kymux: Arc<kymux::Connection>,
    inner: platform::Inner,
}

impl KymuxBackend {
    pub(crate) async fn new(backend_config: KymuxBackendConfig<'_>) -> Result<Self> {
        let mut kymux = start_kymux(&backend_config).await?;

        // Start stream
        let host_config = host_service::start_stream(&backend_config).await?;

        // Play Video and Audio
        let player = platform::create_player(&mut kymux, &backend_config, &host_config).await?;

        // Start input
        let input_service = match (
            backend_config.streaming_config.input.as_ref(),
            host_config.input,
        ) {
            (Some(config), Some(host_config)) => {
                platform::start_input(
                    &kymux,
                    config,
                    host_config.endpoint,
                    backend_config.msg_proxy.clone(),
                )
                .await?
            }
            (None, None) => None,
            _ => {
                error!("Invalid Input config. Should either get a config and an endpoint, or none of them.");
                return Err(Error::InvalidState);
            }
        };

        // Monitor connection closed
        #[allow(clippy::arc_with_non_send_sync)]
        let kymux = Arc::new(kymux);
        let closed_task = platform::KymuxClosedTask::new(kymux_closed_task(
            kymux.clone(),
            backend_config.msg_proxy.clone(),
        ));

        Ok(Self {
            kymux,
            inner: platform::Inner {
                player,
                input_service,
                closed_task: Some(closed_task),
            },
        })
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl BackendDriver for KymuxBackend {
    async fn input_service_connected(&mut self) -> Result<()> {
        self.inner.input_service_connected().await
    }

    async fn input_service_disconnected(&mut self) -> Result<()> {
        self.inner.input_service_disconnected().await
    }

    async fn stop(&mut self) -> Result<()> {
        self.inner.stop().await?;

        let ret = self.kymux.stop().await;
        match ret {
            Ok(_) => {
                info!("Kymux stopped");
            }
            Err(err) => {
                warn!("Failed to stop kymux: {err:?}");
            }
        }

        Ok(())
    }
}
