// Project Kyber: host_service.rs
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

use log::{info, warn};
use reqwest::{self, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::platform::get_cursor_format;
use crate::{CursorFormat, InputSessionConfig};

use super::{HostAudioConfig, HostVideoConfig};

#[derive(Debug, Serialize)]
struct StartRequestVideo {
    port: u16,
    display_id: u32,
    yuv444: bool,
    intra_refresh: Option<bool>,
}

#[derive(Debug, Serialize)]
struct StartRequestAudio {
    port: u16,
}

#[derive(Debug, Serialize)]
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
    pub(crate) gamepad_force_xbox360: Option<bool>,
    pub(crate) clipboard: bool,
}

#[derive(Debug, Serialize)]
struct StartRequest {
    ip: String,
    bitrate: u32,
    video: Option<StartRequestVideo>,
    audio: Option<StartRequestAudio>,
    inputs: Option<StartRequestInput>,
}

#[derive(Debug, Deserialize)]
struct StartResponse {
    session_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct HelloResponse {
    ip: String,
}

#[derive(Debug, Deserialize)]
struct GenerateSdpResponse {
    params: String,
}

pub(crate) async fn hello(client: &HttpClient) -> Result<String> {
    let url = format!("{base_url}/rtp/hello", base_url = client.base_url());

    let response = client.get(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /rtp/hello failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    let response = response.json::<HelloResponse>().await?;
    info!("Hello response: {:?}", response);

    Ok(response.ip)
}

pub(crate) async fn generate_sdp(
    client: &HttpClient,
    yuv444: bool,
    display_id: u32,
) -> Result<String> {
    let url = format!("{base_url}/rtp/generate_sdp", base_url = client.base_url());

    let body = json!({
        "yuv444": yuv444,
        "display_id": display_id,
    });

    let response = client.post(&url).json(&body).send().await?;
    if response.status() != StatusCode::OK {
        warn!("POST /rtp/generate_sdp failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    let response = response.json::<GenerateSdpResponse>().await?;
    info!("Generate SDP response: {:?}", response);

    Ok(response.params)
}

pub(super) async fn start_stream(
    client: &HttpClient,
    local_ip: &str,
    bitrate: u32,
    video: &Option<HostVideoConfig>,
    audio: &Option<HostAudioConfig>,
    input_config: &Option<InputSessionConfig>,
) -> Result<()> {
    let url = format!("{base_url}/rtp/start", base_url = client.base_url());

    let video = video.as_ref().map(|video| StartRequestVideo {
        display_id: video.display_id,
        port: video.port,
        yuv444: video.yuv444,
        intra_refresh: video.intra_refresh,
    });

    let audio = audio
        .as_ref()
        .map(|audio| StartRequestAudio { port: audio.port });

    let inputs = if let Some(input_config) = input_config {
        let cursor_format = get_cursor_format(&input_config.pipeline);
        cursor_format.map(|cursor_format| StartRequestInput {
            cursor_format,
            gamepad_force_xbox360: input_config.gamepad_force_xbox360,
            clipboard: input_config.clipboard,
        })
    } else {
        None
    };

    let body = StartRequest {
        ip: local_ip.into(),
        bitrate,
        video,
        audio,
        inputs,
    };

    info!("Starting stream with parameters {body:?}");
    let response = client.post(&url).json(&body).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /rtp/start failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    let response = response.json::<StartResponse>().await?;
    info!(
        "Session started. SessionId: {}",
        response
            .session_id
            .map(|v| v.to_string())
            .unwrap_or("None".to_string())
    );

    Ok(())
}
