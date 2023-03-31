use log::info;
use reqwest::{self, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::http::HttpClient;

use super::{AudioConfig, VideoConfig};

#[derive(Debug, Serialize)]
struct StartRequestVideo {
    port: u16,
    display_id: u32,
    yuv444: bool,
}

#[derive(Debug, Serialize)]
struct StartRequestAudio {
    port: u16,
}

#[derive(Debug, Serialize)]
struct StartRequest {
    ip: String,
    video: Option<StartRequestVideo>,
    audio: Option<StartRequestAudio>,
    inputs: bool,
}

#[derive(Debug, Deserialize)]
struct StartResponse {
    ip: String,
}

#[derive(Debug, Serialize)]
struct GenerateSdpRequest {
    yuv444: bool,
}

#[derive(Debug, Deserialize)]
struct GenerateSdpResponse {
    params: String,
}

pub(crate) async fn hello(client: &HttpClient) -> Result<String> {
    let url = format!("{base_url}/rtp/hello", base_url = client.base_url());

    let response = client
        .client()
        .get(&url)
        .send()
        .await?
        .json::<StartResponse>()
        .await?;
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

    let response = client
        .client()
        .post(&url)
        .json(&body)
        .send()
        .await?
        .json::<GenerateSdpResponse>()
        .await?;
    info!("Generate SDP response: {:?}", response);

    Ok(response.params)
}

pub(super) async fn start_stream(
    client: &HttpClient,
    local_ip: &str,
    video: &Option<VideoConfig>,
    audio: &Option<AudioConfig>,
    inputs: bool,
) -> Result<()> {
    let url = format!("{base_url}/rtp/start", base_url = client.base_url());

    let video = video.as_ref().map(|video| StartRequestVideo {
        display_id: video.display_id,
        port: video.port,
        yuv444: video.yuv444,
    });

    let audio = audio
        .as_ref()
        .map(|audio| StartRequestAudio { port: audio.port });

    let body = StartRequest {
        ip: local_ip.into(),
        video,
        audio,
        inputs,
    };

    info!("Starting stream with parameters {body:?}");
    let response = client.client().post(&url).json(&body).send().await?;
    info!("Start response: {:?}", response);

    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(Error::StreamStartRejected)
    }
}
