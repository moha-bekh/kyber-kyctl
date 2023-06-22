use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::Result;
use crate::get_display_id;
use crate::http::HttpClient;
use crate::{Display, VideoCodec, VideoSessionConfig};

use super::KymuxConfig;

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
    display_id: u32,
    codec: VideoCodec,
    yuv444: bool,
}

#[derive(Debug, Serialize)]
struct StartRequest {
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    bitrate: u32,

    kymux_config: KymuxConfig,
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
    displays: &Vec<Display>,
    bitrate: u32,
    kymux_config: KymuxConfig,
    video: &Option<VideoSessionConfig>,
    audio: bool,
    inputs: bool,
) -> Result<StartResponse> {
    let url = format!(
        "{base_url}/kymux/start_session",
        base_url = client.base_url()
    );

    let video = video.as_ref().map(|video| StartRequestVideo {
        display_id: get_display_id(displays, video).id,
        codec: video.codec,
        yuv444: video.yuv444,
    });

    let body = StartRequest {
        bitrate,
        kymux_config,
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
