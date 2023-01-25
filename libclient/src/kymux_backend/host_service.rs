use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::Result;
use crate::http::HttpClient;
use crate::VideoCodec;

#[derive(Debug, Deserialize)]
pub(crate) struct StartKymuxResponse {
    pub(crate) port: u16,
}

pub(crate) async fn start_kymux(client: &HttpClient) -> Result<StartKymuxResponse> {
    let url = format!("{base_url}/kymux/start_mux", base_url = client.base_url());

    Ok(client
        .client()
        .post(&url)
        .send()
        .await?
        .json::<StartKymuxResponse>()
        .await?)
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartResponse {
    pub(crate) video_endpoint: u64,
    pub(crate) audio_endpoint: Option<u64>,
    pub(crate) input_endpoint: Option<u64>,
}

pub(crate) async fn start_stream(
    client: &HttpClient,
    video_codec: VideoCodec,
    yuv444: bool,
    audio: bool,
    inputs: bool,
) -> Result<StartResponse> {
    let url = format!(
        "{base_url}/kymux/start_session",
        base_url = client.base_url()
    );

    let video_codec = match video_codec {
        VideoCodec::H264 => "h264",
        VideoCodec::HEVC => "hevc",
        VideoCodec::AV1 => "av1",
    };

    let body = json!({
        "video_codec": video_codec,
        "yuv444": yuv444,
        "audio": audio,
        "inputs": inputs,
    });

    Ok(client
        .client()
        .post(&url)
        .json(&body)
        .send()
        .await?
        .json::<StartResponse>()
        .await?)
}
