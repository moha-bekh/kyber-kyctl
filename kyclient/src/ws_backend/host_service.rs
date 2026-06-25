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

use kyctl_common::ws::{StartRequest, StartRequestInput, StartRequestVideo, StartResponse};

use crate::Result;
use crate::platform::get_cursor_format;
use crate::ws_backend::WsBackendConfig;

pub(super) async fn start_stream(backend_config: &WsBackendConfig<'_>) -> Result<()> {
    let base_url = backend_config.http_client.base_url();
    let url = format!("{base_url}/websocket/start");

    let video = &backend_config.streaming_config.video;
    let video = if !video.is_empty() {
        let video = &video[0];

        Some(StartRequestVideo {
            display_id: video.display_id,
            codec: video.codec.into(),
            yuv444: video.yuv444,
            intra_refresh: video.intra_refresh,
        })
    } else {
        None
    };

    let inputs = if let Some(input_config) = &backend_config.streaming_config.input {
        let cursor_format = get_cursor_format(&input_config.pipeline);
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
        video: video.clone(),
        audio: backend_config.streaming_config.audio.is_some(),
        inputs,
    };

    let response = backend_config
        .http_client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("POST failed: {err}"))?;

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
