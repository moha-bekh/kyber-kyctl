use log::warn;
use reqwest::StatusCode;
use serde::Serialize;

use crate::http::HttpClient;
use crate::{CursorFormat, StreamingSessionConfig, VideoCodec};
use crate::{Error, Result};

#[derive(Clone, Debug, Serialize)]
struct StartRequestVideo {
    display_id: u32,
    codec: VideoCodec,
    yuv444: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
}

#[derive(Clone, Debug, Serialize)]
struct StartRequest {
    bitrate: u32,
    video: Option<StartRequestVideo>,
    audio: bool,
    inputs: Option<StartRequestInput>,
}

pub(super) async fn start_stream(
    http_client: &HttpClient,
    streaming_config: &StreamingSessionConfig,
    cursor_format: Option<CursorFormat>,
) -> Result<()> {
    let base_url = http_client.base_url();
    let url = format!("{base_url}/websocket/start");

    let video = streaming_config
        .video
        .as_ref()
        .map(|video| StartRequestVideo {
            display_id: video.display_id,
            codec: video.codec,
            yuv444: video.yuv444,
        });

    let inputs = cursor_format.map(|cursor_format| StartRequestInput { cursor_format });

    let body = StartRequest {
        bitrate: streaming_config.bitrate,
        video: video.clone(),
        audio: streaming_config.audio,
        inputs,
    };

    let response = http_client.post(&url).json(&body).send().await?;
    if response.status() != StatusCode::OK {
        warn!("POST /websocket/start failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(())
}
