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

mod net_kyproto;
#[cfg(feature = "websocket")]
mod net_kywebsocket;

use async_trait::async_trait;
use kymux::types;

#[cfg(target_family = "wasm")]
use kywebplayer::AudioResumeState;

use crate::error::Result;
use crate::http::HttpClient;
use crate::metrics::MetricsProducer;
use crate::{BackendDriver, ConnectionParams, MessageSender, PeerInfo, StreamingSessionConfig};

pub(crate) mod host_service;
pub(crate) mod metrics;

pub use kyctl_common::kymux::KymuxConfig;

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

enum KyTransport {
    KyProto(net_kyproto::KyProtoTransport),
    #[cfg(feature = "websocket")]
    KyWebSocket,
}

impl KyTransport {
    fn stop(&mut self) {
        // Use a match to avoid a warning if the "websocket" feature is disabled
        match self {
            KyTransport::KyProto(transport) => transport.stop(),
            #[cfg(feature = "websocket")]
            KyTransport::KyWebSocket => {}
        }
    }
}

pub(crate) struct KymuxBackend {
    platform: Box<dyn BackendDriver>,
    transport: KyTransport,
}

impl KymuxBackend {
    pub(crate) async fn new(
        backend_config: KymuxBackendConfig<'_>,
        metrics: Option<&MetricsProducer>,
    ) -> Result<Self> {
        let (transport, endpoints) = match &backend_config.kymux_config {
            KymuxConfig::KyProto(_) => {
                let (transport, endpoint) =
                    net_kyproto::KyProtoTransport::new(&backend_config, metrics).await?;
                let transport = KyTransport::KyProto(transport);
                (transport, endpoint)
            }
            // kyctl_common::KymuxConfig::KyWebSocket must exit even if the
            // "websocket" feature is disabled: it's part of the protocol
            // between the client and the server
            KymuxConfig::KyWebSocket => {
                #[cfg(feature = "websocket")]
                {
                    let endpoint = net_kywebsocket::new(&backend_config).await?;
                    let transport = KyTransport::KyWebSocket;
                    (transport, endpoint)
                }
                #[cfg(not(feature = "websocket"))]
                {
                    log::error!("WebSocket config but WebSocket feature disabled");
                    return Err(crate::error::Error::Rejected);
                }
            }
        };

        let platform = new_platform_backend(&backend_config, endpoints, metrics.cloned()).await?;

        Ok(Self {
            platform,
            transport,
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
        self.transport.stop();
        self.platform.stop().await
    }
}
