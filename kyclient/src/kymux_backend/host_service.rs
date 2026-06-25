// Project Kyber: host_service.rs
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

use log::{info, warn};
use serde::Serialize;

use kyctl_common::kymux::{
    StartKymuxResponse, StartRequestInput, StartRequestVideo, StartSessionResponse,
};

use crate::CursorFormat;
use crate::error::Result;
use crate::http::HttpClient;
use crate::platform::get_cursor_format;

use super::{KymuxBackendConfig, KymuxConfig, KymuxTransportBackend};

pub(crate) use kyctl_common::kymux::StartKymuxCertificateHash;

#[derive(Debug, Serialize)]
struct StartMuxRequest {
    public_streamer_port: u16,
    backend: KymuxTransportBackend,
}

pub(crate) async fn start_kymux(
    client: &HttpClient,
    backend: KymuxTransportBackend,
    streamer_port: u16,
) -> Result<StartKymuxResponse> {
    let url = format!("{base_url}/kymux/start_mux", base_url = client.base_url());

    let body = StartMuxRequest {
        public_streamer_port: streamer_port,
        backend,
    };

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("POST failed: {err}"))?;

    Ok(response.json().await?)
}

#[derive(Debug, Serialize)]
struct StartRequest {
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    bitrate: u32,

    kymux_config: KymuxConfig,
    video: Option<Vec<StartRequestVideo>>,
    audio: bool,
    inputs: Option<StartRequestInput>,
    metrics: bool,
}

pub(crate) async fn start_stream(
    backend_config: &KymuxBackendConfig<'_>,
    metrics: bool,
) -> Result<StartSessionResponse> {
    let url = format!(
        "{base_url}/kymux/start_session",
        base_url = backend_config.http_client.base_url()
    );

    let video = &backend_config.streaming_config.video;
    let video = if !video.is_empty() {
        Some(
            video
                .iter()
                .map(|config| StartRequestVideo {
                    display_id: config.display_id,
                    codec: config.codec.into(),
                    yuv444: config.yuv444,
                    intra_refresh: config.intra_refresh,
                })
                .collect(),
        )
    } else {
        None
    };

    let inputs = if let Some(input_config) = &backend_config.streaming_config.input {
        let cursor_format = get_cursor_format(&input_config.pipeline).or_else(|| {
            warn!("No Cursor format found. Force raw");
            Some(CursorFormat::Raw)
        });
        cursor_format.map(|cursor_format| StartRequestInput {
            cursor_format: cursor_format.into(),
            gamepad_force_xbox360: input_config.gamepad_force_xbox360,
            clipboard: input_config.clipboard,
        })
    } else {
        None
    };

    let body = StartRequest {
        bitrate: backend_config.streaming_config.bitrate,
        kymux_config: backend_config.kymux_config.clone(),
        video,
        audio: backend_config.streaming_config.audio.is_some(),
        inputs,
        metrics,
    };

    let response = backend_config
        .http_client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("POST failed: {err}"))?;

    let response = response.json::<StartSessionResponse>().await?;
    info!(
        "Session started. SessionId: {}",
        response
            .session_id
            .map(|v| v.to_string())
            .unwrap_or("None".to_string())
    );

    let requested_clipboard = body.inputs.as_ref().is_some_and(|inputs| inputs.clipboard);
    let got_clipboard = response
        .input
        .as_ref()
        .is_some_and(|inputs| inputs.clipboard);

    if requested_clipboard && !got_clipboard {
        warn!("Requested clipboard forwarding but server did not enable it.");
    }

    Ok(response)
}
