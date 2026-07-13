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
    let host = backend_config.http_client.host();

    host_service::start_stream(backend_config, false).await?;

    let endpoints = create_platform_endpoints(host, backend_config).await?;
    Ok(endpoints)
}

async fn create_platform_endpoints(
    host: &str,
    backend_config: &KymuxBackendConfig<'_>,
) -> Result<PlatformEndpoints> {
    let video = {
        let mut vec = Vec::new();
        if !backend_config.streaming_config.video.is_empty() {
            let url = format!("wss://{host}/websocket/video");
            let conn = kywebsocket::Connection::websocket_js_connect(&url).await?;
            let endpoint = conn.into_channel().into_endpoint();
            vec.push(endpoint);
        }
        vec
    };
    let audio = if backend_config.streaming_config.audio.is_some() {
        let url = format!("wss://{host}/websocket/audio");
        let conn = kywebsocket::Connection::websocket_js_connect(&url).await?;
        let endpoint = conn.into_channel().into_endpoint();
        Some(endpoint)
    } else {
        None
    };
    let input = if backend_config.streaming_config.input.is_some() {
        let url = format!("wss://{host}/websocket/inputs");
        let conn = kywebsocket::Connection::websocket_js_connect(&url).await?;
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
