// Project Kyber: mod.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

#[cfg(not(target_family = "wasm"))]
mod desktop;

#[cfg(target_family = "wasm")]
mod web;

use async_trait::async_trait;
use kymux::{kynet, kyproto, types};
use log::{error, info, warn};
use std::collections::HashMap;

#[cfg(target_family = "wasm")]
use kywebplayer::AudioResumeState;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::metrics::MetricsProducer;
use crate::runtime::{self, Duration};
use crate::util::*;
use crate::{
    BackendDriver, ConnectionParams, DataPlaneCloseReason, Message, MessageSender, PeerInfo,
    StreamingSessionConfig,
};
use kyctl_common::kymux::StartSessionResponse;

pub(crate) mod host_service;
pub(crate) mod metrics;

pub use kyctl_common::kymux::{AudioProtocol, KymuxConfig, VideoProtocol};

#[allow(dead_code)]
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct KymuxBackendConfig<'a> {
    pub(crate) conn_params: &'a ConnectionParams,
    /// Used by the QUIC (desktop) path in `connect_quinn()`.
    #[allow(dead_code)]
    pub(crate) peer_info: &'a PeerInfo,
    pub(crate) http_client: &'a HttpClient,
    pub(crate) streaming_config: StreamingSessionConfig,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) msg_sender: MessageSender,
}

#[cfg(not(target_family = "wasm"))]
async fn new_platform_backend(
    backend_config: &KymuxBackendConfig<'_>,
    endpoints: PlatformEndpoints,
    metrics: Option<MetricsProducer>,
) -> Result<Box<dyn BackendDriver>> {
    let backend = desktop::KymuxDesktop::new(backend_config, endpoints, metrics).await?;
    Ok(Box::new(backend))
}

#[cfg(target_family = "wasm")]
async fn new_platform_backend(
    backend_config: &KymuxBackendConfig<'_>,
    endpoints: PlatformEndpoints,
    metrics: Option<MetricsProducer>,
) -> Result<Box<dyn BackendDriver>> {
    let backend = web::KymuxWeb::new(backend_config, endpoints, metrics).await?;
    Ok(Box::new(backend))
}

pub(crate) struct PlatformEndpoints {
    video: Vec<types::VideoClientEndpoint>,
    audio: Option<types::AudioClientEndpoint>,
    input: Option<types::InputEndpoint>,
}

pub(crate) struct KymuxBackend {
    platform: Box<dyn BackendDriver>,
    connection: KyArc<kyproto::Connection>,
    closed_task: Option<runtime::Task>,
}

impl KymuxBackend {
    pub(crate) async fn new(
        backend_config: KymuxBackendConfig<'_>,
        metrics: Option<&MetricsProducer>,
    ) -> Result<Self> {
        let connection = start_kymux(&backend_config).await?;
        let connection = KyArc::new(connection);

        let has_metrics = metrics.is_some();

        // Start stream
        let host_config = host_service::start_stream(&backend_config, has_metrics).await?;

        if let Some(metrics) = metrics {
            start_metrics(&connection, &host_config, metrics.clone()).await?;
        }

        let msg_sender = backend_config.msg_sender.clone();

        let endpoints = create_platform_endpoints(&connection, &backend_config, &host_config)?;
        let platform = new_platform_backend(&backend_config, endpoints, metrics.cloned()).await?;

        // Monitor connection closed
        let closed_task = kymux_closed_task(connection.clone(), msg_sender);

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

    #[cfg(target_family = "wasm")]
    fn audio_try_resume(&self) -> AudioResumeState {
        self.platform.audio_try_resume()
    }

    #[cfg(target_family = "wasm")]
    async fn audio_resume(&self) -> Result<()> {
        self.platform.audio_resume().await
    }

    fn input_service_connected(&mut self) -> Result<()> {
        self.platform.input_service_connected()
    }

    fn input_service_disconnected(&mut self) -> Result<()> {
        self.platform.input_service_disconnected()
    }

    async fn stop(&mut self) -> Result<()> {
        self.closed_task = None;

        self.connection.close();
        self.platform.stop().await
    }
}

fn kymux_closed_task(
    connection: KyArc<kyproto::Connection>,
    msg_sender: MessageSender,
) -> runtime::Task {
    runtime::Task::spawn(async move {
        let reason = match connection.closed().await {
            Ok(()) => DataPlaneCloseReason::Closed,
            Err(_) => DataPlaneCloseReason::Error,
        };

        let _ = msg_sender.send(Message::DataPlaneClosed { reason });
    })
}

async fn start_kymux(backend_config: &KymuxBackendConfig<'_>) -> Result<kyproto::Connection> {
    // Request remote Kymux start
    let start_response =
        host_service::start_kymux(backend_config.http_client, backend_config.conn_params.port)
            .await?;

    // Start Kymux connection
    let auth = kyproto::ClientAuth::new(&start_response.token).map_err(|_| {
        warn!("Got an invalid kymux token from controller");
        Error::KymuxInvalidToken
    })?;

    let kynet = {
        #[cfg(not(target_family = "wasm"))]
        {
            info!(
                "Connect using Quic backend. Port: {port}",
                port = start_response.port
            );

            connect_quinn(
                backend_config.peer_info,
                &backend_config.conn_params.host,
                start_response.port,
                start_response.certificate_hash,
            )
            .await
        }

        #[cfg(target_family = "wasm")]
        {
            info!(
                "Connect using Webtransport backend. Port: {port}",
                port = start_response.port
            );

            let host_str = crate::http::format_host_for_url(
                &backend_config.conn_params.host,
                start_response.port,
            );
            let url = format!("https://{host_str}");

            connect_webtransport_js(&url, start_response.certificate_hash).await
        }
    }
    .map_err(|err| {
        warn!("Failed to connect to DataPlane: {err:?}");
        Error::DataPlaneConnectFailed
    })?;

    let kyproto = kyproto::Connection::connect_with_auth(kynet, &auth)
        .await
        .map_err(|err| {
            warn!("Failed to connect KyProto: {err:?}");
            Error::DataPlaneConnectFailed
        })?;

    Ok(kyproto)
}

#[cfg(not(target_family = "wasm"))]
async fn connect_quinn(
    peer_info: &PeerInfo,
    server_name: &str,
    port: u16,
    certificate_hash: Option<host_service::StartKymuxCertificateHash>,
) -> Result<kynet::Connection> {
    let certificate_hash = certificate_hash.map(|h| h.hash);

    let options = kynet::quinn::QuinnClientOptions {
        keep_alive_interval: Some(KEEP_ALIVE_INTERVAL),
        certificate_hash,
        ..Default::default()
    };

    let addr = std::net::SocketAddr::new(peer_info.addr, port);
    let conn = kynet::Connection::quinn_connect(addr, server_name, None, &options).await?;
    Ok(conn)
}

#[cfg(target_family = "wasm")]
async fn connect_webtransport_js(
    url: &str,
    certificate_hash: Option<host_service::StartKymuxCertificateHash>,
) -> Result<kynet::Connection> {
    use kynet::webtransport_js as js;

    let server_certificate_hashes = if let Some(c) = certificate_hash {
        Some(vec![js::WebTransportJSHash::new_from_hex(
            &c.algorithm,
            &c.hash,
        )?])
    } else {
        None
    };

    let options = js::WebTransportJSOptions {
        congestion_control: js::WebTransportJSCongestionControl::LowLatency,
        require_unreliable: true,
        server_certificate_hashes,
    };

    let conn = kynet::Connection::webtransport_js_connect(url, &options).await?;
    Ok(conn)
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
) -> Result<Vec<types::VideoClientEndpoint>> {
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
        .unwrap_or(VideoProtocol::Reliable);
    info!("Using video protocol: {video_protocol:?}");

    let mut video_endpoints = vec![];
    for config in video_config {
        let endpoint_id = *endpoint_list.get(&config.display_id).ok_or_else(|| {
            warn!("No endpoint found for display {}", config.display_id);
            Error::Rejected
        })?;

        let endpoint = connection.connect_video_endpoint(endpoint_id, video_protocol.into())?;
        video_endpoints.push(endpoint);
    }

    Ok(video_endpoints)
}

fn create_audio_endpoint(
    connection: &kyproto::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<Option<types::AudioClientEndpoint>> {
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
        .unwrap_or(AudioProtocol::Reliable);
    info!("Using audio protocol: {audio_protocol:?}");
    let endpoint_id = host_config.endpoint;
    let endpoint = connection.connect_audio_endpoint(endpoint_id, audio_protocol.into())?;
    Ok(Some(endpoint))
}

fn create_input_endpoint(
    connection: &kyproto::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<Option<types::InputEndpoint>> {
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
