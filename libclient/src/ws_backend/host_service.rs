use log::{info, warn};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::platform::get_cursor_format;
use crate::ws_backend::WsBackendConfig;
use crate::{CursorFormat, VideoCodec};
use crate::{Error, Result};

#[derive(Clone, Debug, Serialize)]
struct StartRequestVideo {
    display_id: u32,
    codec: VideoCodec,
    yuv444: bool,
    intra_refresh: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
    pub(crate) gamepad_force_xbox360: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct StartRequest {
    bitrate: u32,
    video: Option<StartRequestVideo>,
    audio: bool,
    inputs: Option<StartRequestInput>,
}

#[derive(Debug, Deserialize)]
struct StartResponse {
    session_id: Option<u32>,
}

pub(super) async fn start_stream(backend_config: &WsBackendConfig<'_>) -> Result<()> {
    let base_url = backend_config.http_client.base_url();
    let url = format!("{base_url}/websocket/start");

    let video = &backend_config.streaming_config.video;
    let video = if !video.is_empty() {
        let video = &video[0];

        Some(StartRequestVideo {
            display_id: video.display_id,
            codec: video.codec,
            yuv444: video.yuv444,
            intra_refresh: video.intra_refresh,
        })
    } else {
        None
    };

    let inputs = if let Some(input_config) = &backend_config.streaming_config.input {
        let cursor_format = get_cursor_format(&input_config.pipeline);
        cursor_format.map(|cursor_format| StartRequestInput {
            cursor_format,
            gamepad_force_xbox360: input_config.gamepad_force_xbox360,
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
        .await?;
    if response.status() != StatusCode::OK {
        warn!("POST /websocket/start failed: {}", response.status());
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
