use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::Result;
use crate::http::HttpClient;
use crate::{VideoCodec, VideoSessionConfig};

#[derive(Debug, Deserialize)]
pub(crate) struct StartKymuxResponse {
    pub(crate) port: u16,
}

pub(crate) async fn start_kymux(
    client: &HttpClient,
    streamer_port: u16,
) -> Result<StartKymuxResponse> {
    let url = format!("{base_url}/kymux/start_mux", base_url = client.base_url());

    let body = json!({
        "public_streamer_port": streamer_port,
    });

    Ok(client
        .client()
        .post(&url)
        .json(&body)
        .send()
        .await?
        .json::<StartKymuxResponse>()
        .await?)
}

#[derive(Debug, Serialize)]
struct StartRequestVideo {
    codec: VideoCodec,
    yuv444: bool,
}

#[derive(Debug, Serialize)]
struct StartRequest {
    video: Option<StartRequestVideo>,
    audio: bool,
    inputs: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartResponse {
    pub(crate) video_endpoint: Option<u64>,
    pub(crate) audio_endpoint: Option<u64>,
    pub(crate) input_endpoint: Option<u64>,
}

pub(crate) async fn start_stream(
    client: &HttpClient,
    video: &Option<VideoSessionConfig>,
    audio: bool,
    inputs: bool,
) -> Result<StartResponse> {
    let url = format!(
        "{base_url}/kymux/start_session",
        base_url = client.base_url()
    );

    let video = video.as_ref().map(|video| StartRequestVideo {
        codec: video.codec,
        yuv444: video.yuv444,
    });

    let body = StartRequest {
        video,
        audio,
        inputs,
    };

    Ok(client
        .client()
        .post(&url)
        .json(&body)
        .send()
        .await?
        .json::<StartResponse>()
        .await?)
}
