#[cfg_attr(feature = "desktop", path = "./desktop.rs")]
#[cfg_attr(feature = "web", path = "./web.rs")]
mod platform;

use async_trait::async_trait;
use log::{error, info, warn};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::platform::get_cursor_format;
use crate::{
    BackendDriver, ConnectionParams, Display, MessageProxy, PlayerConfig, StreamingSessionConfig,
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

async fn start_kymux(
    conn_params: &ConnectionParams,
    client: &HttpClient,
    backend: KymuxTransportBackend,
) -> Result<kymux::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(client, backend, conn_params.port).await?;

    // Start Kymux connection
    let connection = match backend {
        #[cfg(feature = "desktop")]
        KymuxTransportBackend::Quic => {
            info!("Connect using Quic backend");
            platform::connect_quinn(conn_params, start_response.port).await?
        }
        #[cfg(feature = "web")]
        KymuxTransportBackend::Webtransport => {
            info!("Connect using Webtransport backend");

            let Some(certificate_hash) = start_response.certificate_hash else {
                error!("No certificate hash returned");
                return Err(Error::Rejected);
            };

            platform::connect_webtransport(client.base_url().into(), certificate_hash).await?
        }
    };

    Ok(connection)
}

pub(crate) struct KymuxBackend {
    kymux: kymux::Connection,
    inner: platform::Inner,
}

impl KymuxBackend {
    pub(crate) async fn new(
        conn_params: &ConnectionParams,
        http_client: &HttpClient,
        displays: &Vec<Display>,
        player_config: &PlayerConfig,
        streaming_config: &StreamingSessionConfig,
        kymux_config: KymuxConfig,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        let mut kymux = start_kymux(conn_params, http_client, kymux_config.backend).await?;

        let cursor_format = if let Some(input_config) = streaming_config.input.as_ref() {
            get_cursor_format(&input_config.pipeline)
        } else {
            None
        };

        // Start stream
        let start_resp = host_service::start_stream(
            http_client,
            displays,
            streaming_config.bitrate,
            kymux_config.clone(),
            &streaming_config.video,
            streaming_config.audio,
            cursor_format,
        )
        .await?;

        // Play Video and Audio
        let player = platform::create_player(
            &mut kymux,
            &kymux_config,
            player_config,
            streaming_config,
            &start_resp.video_endpoint,
            &start_resp.audio_endpoint,
            msg_proxy.clone(),
        )
        .await?;

        // Start input
        let input_service = match (streaming_config.input.as_ref(), start_resp.input_endpoint) {
            (Some(config), Some(endpoint)) => {
                platform::start_input(&kymux, config, endpoint, msg_proxy.clone()).await?
            }
            (None, None) => None,
            _ => {
                error!("Invalid Input config. Should either get a config and an endpoint, or none of them.");
                return Err(Error::InvalidState);
            }
        };

        Ok(Self {
            kymux,
            inner: platform::Inner {
                player,
                input_service,
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
