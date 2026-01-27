// Project Kyber: mod.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

#[cfg(feature = "desktop")]
mod desktop;

#[cfg(feature = "web")]
mod web;

use async_trait::async_trait;
use kymux::kyproto;
use log::{error, info, warn};
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::oneshot;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::metrics::MetricsProducer;
use crate::runtime::{self, Duration};
use crate::util::*;
use crate::{
    BackendDriver, ConnectionParams, DataPlaneCloseReason, Message, MessageSender,
    StreamingSessionConfig,
};
use host_service::StartSessionResponse;

pub(crate) mod host_service;
pub(crate) mod metrics;

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
    pub video_protocol: Option<kyproto::VideoProtocol>,
    pub audio_protocol: Option<kyproto::AudioProtocol>,
}

pub(crate) struct KymuxBackendConfig<'a> {
    pub(crate) conn_params: &'a ConnectionParams,
    pub(crate) http_client: &'a HttpClient,
    pub(crate) streaming_config: StreamingSessionConfig,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) msg_sender: MessageSender,
}

#[cfg(feature = "desktop")]
async fn new_platform_backend(
    backend_config: &KymuxBackendConfig<'_>,
    endpoints: PlatformEndpoints,
    metrics: bool,
) -> Result<Box<dyn BackendDriver>> {
    let backend = desktop::KymuxDesktop::new(backend_config, endpoints, metrics).await?;
    Ok(Box::new(backend))
}

#[cfg(feature = "web")]
async fn new_platform_backend(
    backend_config: &KymuxBackendConfig<'_>,
    endpoints: PlatformEndpoints,
    metrics: bool,
) -> Result<Box<dyn BackendDriver>> {
    let backend = web::KymuxWeb::new(backend_config, endpoints, metrics).await?;
    Ok(Box::new(backend))
}

pub(crate) struct PlatformEndpoints {
    video: Vec<kyproto::VideoClientEndpoint>,
    audio: Option<kyproto::AudioClientEndpoint>,
    input: Option<kyproto::InputEndpoint>,
}

pub(crate) struct KymuxBackend {
    platform: Box<dyn BackendDriver>,
    connection: KyArc<kyproto::Connection>,
    closed_task: Option<KymuxClosedTask>,
}

impl KymuxBackend {
    pub(crate) async fn new(
        backend_config: KymuxBackendConfig<'_>,
        metrics: Option<MetricsProducer>,
    ) -> Result<Self> {
        let connection = start_kymux(&backend_config).await?;
        let connection = KyArc::new(connection);

        let has_metrics = metrics.is_some();

        // Start stream
        let host_config = host_service::start_stream(&backend_config, has_metrics).await?;

        if let Some(metrics) = metrics {
            start_metrics(&connection, &host_config, metrics).await?;
        }

        let msg_sender = backend_config.msg_sender.clone();

        let endpoints = create_platform_endpoints(&connection, &backend_config, &host_config)?;
        let platform = new_platform_backend(&backend_config, endpoints, has_metrics).await?;

        // Monitor connection closed
        let closed_task = KymuxClosedTask::new(connection.clone(), msg_sender);

        Ok(Self {
            platform,
            connection,
            closed_task: Some(closed_task),
        })
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl BackendDriver for KymuxBackend {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        self.platform.audio_set_volume(volume)
    }

    fn audio_set_mute(&self, mute: bool) -> Result<()> {
        self.platform.audio_set_mute(mute)
    }

    async fn input_service_connected(&mut self) -> Result<()> {
        self.platform.input_service_connected().await
    }

    async fn input_service_disconnected(&mut self) -> Result<()> {
        self.platform.input_service_disconnected().await
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(closed_task) = self.closed_task.take() {
            let ret = closed_task.stop();
            if let Err(err) = ret {
                warn!("Failed to stop KymuxClosedTask: {err:?}");
            }
        }

        self.connection.close();

        self.platform.stop().await
    }
}

struct KymuxClosedTask {
    stop_tx: oneshot::Sender<()>,
}

impl KymuxClosedTask {
    fn new(connection: KyArc<kyproto::Connection>, msg_sender: MessageSender) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        runtime::spawn(async move {
            tokio::select! {
                _ = stop_rx => {}
                ret = connection.closed() => {
                    let reason = match ret {
                        Ok(()) => DataPlaneCloseReason::Closed,
                        Err(_) => DataPlaneCloseReason::Error,
                    };

                    let ret = msg_sender.send_async(Message::DataPlaneClosed{ reason }).await;
                    if let Err(err) = ret {
                        warn!("Fail to send DataPlaneClosed: {err:?}");
                    }
                }
            }
        });

        Self { stop_tx }
    }

    fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("ClosedTask already stopped");
        }

        Ok(())
    }
}

async fn start_kymux(backend_config: &KymuxBackendConfig<'_>) -> Result<kyproto::Connection> {
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

            let auth = kymux::kyproto::ClientAuth::new(&start_response.token).map_err(|_| {
                warn!("Got an invalid kymux token from controller");
                Error::KymuxInvalidToken
            })?;

            connect_quinn(
                backend_config.conn_params,
                start_response.port,
                &auth,
                start_response.certificate_hash,
            )
            .await
        }
        #[cfg(feature = "web")]
        KymuxTransportBackend::Webtransport => {
            info!(
                "Connect using Webtransport backend. Port: {port}",
                port = start_response.port
            );

            let url = format!(
                "https://{host}:{port}",
                host = backend_config.conn_params.host,
                port = start_response.port
            );

            let auth = kymux::kyproto::ClientAuth::new(&start_response.token).map_err(|_| {
                warn!("Got an invalid kymux token from controller");
                Error::KymuxInvalidToken
            })?;

            connect_webtransport(url, start_response.certificate_hash, &auth).await
        }
    };

    ret.map_err(|err| {
        warn!("Failed to connect to DataPlane: {err:?}");
        Error::DataPlaneConnectFailed
    })
}

#[cfg(feature = "desktop")]
async fn connect_quinn(
    conn_params: &ConnectionParams,
    port: u16,
    auth: &kyproto::ClientAuth,
    certificate_hash: Option<host_service::StartKymuxCertificateHash>,
) -> Result<kyproto::Connection> {
    // Extract certificate hash if provided (for self-signed WebTransport certificates)
    let certificate_hash = certificate_hash.map(|h| h.hash);

    let roots = if let Some(tls_config) = &conn_params.tls_config {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(tls_config.root_certificate.clone().into())
            .map_err(|err| {
                warn!("Failed to crate RootCertStore: {err:?}");
                Error::Rejected
            })?;

        Some(roots)
    } else {
        None
    };

    let server_name = if let Some(tls_host) = &conn_params.tls_host {
        tls_host.clone()
    } else {
        conn_params.host.clone()
    };

    Ok(kymux::connect_with_auth(
        kymux::ClientConfig::Quic {
            addr: std::net::SocketAddr::new(conn_params.addr.ip(), port),
            roots,
            server_name,
            certificate_hash,
        },
        auth,
    )
    .await?)
}

#[cfg(feature = "web")]
async fn connect_webtransport(
    url: String,
    certificate_hash: Option<host_service::StartKymuxCertificateHash>,
    auth: &kyproto::ClientAuth,
) -> Result<kyproto::Connection> {
    let certificate_hash = certificate_hash.map(|c| kymux::WebTransportCertificateHash {
        hash_algorithm: c.algorithm,
        hash: c.hash,
    });

    Ok(kymux::connect_with_auth(
        kymux::ClientConfig::WebTransport {
            url,
            certificate_hash,
        },
        auth,
    )
    .await?)
}

async fn start_metrics(
    connection: &kyproto::Connection,
    host_config: &StartSessionResponse,
    metrics: MetricsProducer,
) -> Result<()> {
    let metrics2 = metrics.clone();
    let metrics3 = metrics.clone();

    metrics::start_connection_stats_poller(connection, metrics.clone(), Duration::from_secs(5))
        .await;

    let clock_sync_id = host_config.clock_sync.as_ref().unwrap().endpoint;
    metrics::start_pinger(connection, clock_sync_id, metrics2, Duration::from_secs(10)).await?;

    let remote_network_stats_id = host_config.metrics.as_ref().unwrap().endpoint;
    metrics::start_remote_metrics_receiver(connection, remote_network_stats_id, metrics3).await?;

    if let Some(video) = &host_config.video {
        let endpoints = video
            .iter()
            .filter_map(|v| v.metrics_endpoint.map(|endpoint| (v.display_id, endpoint)));
        for (display_id, endpoint) in endpoints {
            metrics::start_remote_video_metrics_receiver(
                connection,
                display_id,
                endpoint,
                metrics.clone(),
            )
            .await?;
        }
    }

    Ok(())
}

fn create_video_endpoints(
    connection: &kyproto::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<Vec<kyproto::VideoClientEndpoint>> {
    let video_config = &backend_config.streaming_config.video;
    if video_config.is_empty() {
        return Ok(vec![]);
    }

    let Some(host_config) = &host_config.video else {
        error!("Missing video config");
        return Err(Error::InvalidState);
    };

    let endpoint_list: HashMap<u32, u16> = host_config
        .iter()
        .map(|video| (video.display_id, video.endpoint))
        .collect();

    let video_protocol = backend_config
        .kymux_config
        .video_protocol
        .unwrap_or(kyproto::VideoProtocol::Reliable);
    info!("Using video protocol: {video_protocol:?}");

    let mut video_endpoints = vec![];
    for config in video_config {
        let endpoint_id = *endpoint_list.get(&config.display_id).ok_or_else(|| {
            warn!("No endpoint found for display {}", config.display_id);
            Error::Rejected
        })?;

        let endpoint = connection.connect_video_endpoint(endpoint_id, video_protocol)?;
        video_endpoints.push(endpoint);
    }

    Ok(video_endpoints)
}

fn create_audio_endpoint(
    connection: &kyproto::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<Option<kyproto::AudioClientEndpoint>> {
    if backend_config.streaming_config.audio.is_none() {
        return Ok(None);
    };

    let Some(host_config) = &host_config.audio else {
        error!("Missing audio config");
        return Err(Error::InvalidState);
    };

    let audio_protocol = backend_config
        .kymux_config
        .audio_protocol
        .unwrap_or(kyproto::AudioProtocol::Reliable);
    info!("Using audio protocol: {audio_protocol:?}");
    let endpoint_id = host_config.endpoint;
    let endpoint = connection.connect_audio_endpoint(endpoint_id, audio_protocol)?;
    Ok(Some(endpoint))
}

fn create_input_endpoint(
    connection: &kyproto::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<Option<kyproto::InputEndpoint>> {
    if backend_config.streaming_config.input.is_none() {
        return Ok(None);
    }

    let Some(host_config) = &host_config.input else {
        error!("Missing input config");
        return Err(Error::InvalidState);
    };

    let endpoint = connection.connect_input_endpoint(host_config.endpoint)?;
    Ok(Some(endpoint))
}

fn create_platform_endpoints(
    connection: &kyproto::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<PlatformEndpoints> {
    let video = create_video_endpoints(connection, backend_config, host_config)?;
    let audio = create_audio_endpoint(connection, backend_config, host_config)?;
    let input = create_input_endpoint(connection, backend_config, host_config)?;
    Ok(PlatformEndpoints {
        video,
        audio,
        input,
    })
}
