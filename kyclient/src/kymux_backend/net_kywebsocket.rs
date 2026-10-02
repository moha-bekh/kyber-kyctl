// Project Kyber: net_kywebsocket.rs
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

use super::host_service;
use super::{KymuxBackendConfig, PlatformEndpoints};
use crate::error::Result;

use kymux::kywebsocket;

pub async fn new(backend_config: &KymuxBackendConfig<'_>) -> Result<PlatformEndpoints> {
    host_service::start_stream(backend_config, false).await?;

    let endpoints = create_platform_endpoints(backend_config).await?;
    Ok(endpoints)
}

async fn connect(
    backend_config: &KymuxBackendConfig<'_>,
    path: &str,
) -> Result<kywebsocket::Connection> {
    let host = backend_config.http_client.host();
    let url = format!("wss://{host}{path}");

    let conn = {
        #[cfg(not(target_family = "wasm"))]
        {
            let Some(hash) = &backend_config.peer_info.certificate_hash else {
                return Err(crate::Error::TlsMissingCertificateHash);
            };

            // Authenticate with the current session cookie (the cookie jar is
            // shared with the HTTP client)
            let tls_config = kywebsocket::tungstenite::TlsConfig::ServerHash(hash.clone());
            let mut headers = kywebsocket::tungstenite::HeaderMap::new();
            if let Some(cookie) =
                crate::platform::http::cookie_header(backend_config.conn_params, path)
            {
                headers.insert(kywebsocket::tungstenite::COOKIE, cookie);
            }

            let addr = backend_config.peer_info.addr;
            kywebsocket::Connection::tungstenite_connect(&url, Some(addr), tls_config, headers)
                .await
        }

        #[cfg(target_family = "wasm")]
        {
            kywebsocket::Connection::websocket_js_connect(&url).await
        }
    }?;

    Ok(conn)
}

async fn create_platform_endpoints(
    backend_config: &KymuxBackendConfig<'_>,
) -> Result<PlatformEndpoints> {
    let video = {
        let mut vec = Vec::new();
        if !backend_config.streaming_config.video.is_empty() {
            let conn = connect(backend_config, "/websocket/video").await?;
            let endpoint = conn.into_channel().into_endpoint();
            vec.push(endpoint);
        }
        vec
    };
    let audio = if backend_config.streaming_config.audio.is_some() {
        let conn = connect(backend_config, "/websocket/audio").await?;
        let endpoint = conn.into_channel().into_endpoint();
        Some(endpoint)
    } else {
        None
    };
    let input = if backend_config.streaming_config.input.is_some() {
        let conn = connect(backend_config, "/websocket/inputs").await?;
        let endpoint = conn.into_channel().into_endpoint();
        Some(endpoint)
    } else {
        None
    };
    Ok(PlatformEndpoints {
        video,
        audio,
        input,
    })
}
