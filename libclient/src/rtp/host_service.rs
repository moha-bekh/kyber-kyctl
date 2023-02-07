use log::info;
use reqwest::{self, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};
use crate::http::HttpClient;

#[derive(Debug, Deserialize, Serialize)]
struct StartResponse {
    ip: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GenerateSdpRequest {
    yuv444: bool,
}

#[derive(Debug, Deserialize, Serialize)]
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

pub(crate) async fn generate_sdp(client: &HttpClient, yuv444: bool) -> Result<String> {
    let url = format!("{base_url}/rtp/generate_sdp", base_url = client.base_url());

    let body = json!({
        "yuv444": yuv444,
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

pub(crate) async fn start_stream(
    client: &HttpClient,
    local_ip: &str,
    video_port: u16,
    yuv444: bool,
    audio: bool,
    inputs: bool,
) -> Result<()> {
    let url = format!("{base_url}/rtp/start", base_url = client.base_url());

    let body = json!({
        "ip": local_ip,
        "port": video_port,
        "yuv444": yuv444,
        "audio": audio,
        "inputs": inputs,
    });

    let response = client.client().post(&url).json(&body).send().await?;
    info!("Start response: {:?}", response);

    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(Error::StreamStartRejected)
    }
}
