use log::{info, warn};
use reqwest::{self, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::CursorFormat;

use super::{HostAudioConfig, HostVideoConfig};

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
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
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

    let response = client.client().get(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /rtp/hello failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    let response = response.json::<StartResponse>().await?;
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

    let response = client.client().post(&url).json(&body).send().await?;
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
    inputs: Option<CursorFormat>,
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

    let inputs = inputs.map(|cursor_format| StartRequestInput { cursor_format });

    let body = StartRequest {
        ip: local_ip.into(),
        bitrate,
        video,
        audio,
        inputs,
    };

    info!("Starting stream with parameters {body:?}");
    let response = client.client().post(&url).json(&body).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /rtp/start failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(())
}
