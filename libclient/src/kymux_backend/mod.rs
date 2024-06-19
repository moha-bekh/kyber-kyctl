#[cfg_attr(feature = "desktop", path = "./desktop.rs")]
#[cfg_attr(feature = "web", path = "./web.rs")]
mod platform;

use std::sync::Arc;

use async_trait::async_trait;
use log::{error, info, warn};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::kymux_backend::host_service::StartSessionInputResponse;
use crate::kymux_backend::platform::InputService;
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
    pub audio_protocol: Option<kymux::AudioProtocol>,
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
            info!(
                "Connect using Quic backend. Port: {port}",
                port = start_response.port
            );
            platform::connect_quinn(backend_config.conn_params, start_response.port).await
        }
        #[cfg(feature = "web")]
        KymuxTransportBackend::Webtransport => {
            info!(
                "Connect using Webtransport backend. Port: {port}",
                port = start_response.port
            );

            let Some(certificate_hash) = start_response.certificate_hash else {
                error!("No certificate hash returned");
                return Err(Error::Rejected);
            };

            let url = format!(
                "https://{host}:{port}",
                host = backend_config.conn_params.host,
                port = start_response.port
            );

            platform::connect_webtransport(url, certificate_hash).await
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
        let input_service = Self::start_inputs(&backend_config, &kymux, host_config.input).await?;

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

    async fn start_inputs(
        backend_config: &KymuxBackendConfig<'_>,
        kymux: &kymux::Connection,
        host_config: Option<StartSessionInputResponse>,
    ) -> Result<Option<InputService>> {
        let (input_config, endpoint) = match (&backend_config.streaming_config.input, host_config) {
            (Some(input_config), Some(host_config)) => (input_config, host_config.endpoint),
            (None, None) => return Ok(None),
            _ => {
                error!("Invalid Input config. Should either get a config and an endpoint, or none of them.");
                return Err(Error::InvalidState);
            }
        };

        let input_service = platform::start_input(
            kymux,
            input_config,
            endpoint,
            backend_config.msg_proxy.clone(),
        )
        .await?;

        Ok(input_service)
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
