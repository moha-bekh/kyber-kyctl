use log::info;
use reqwest::{self, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{Error, Result};

const STREAMER_PORT: u16 = 8080;

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

pub async fn hello(streamer_ip: &str) -> Result<String> {
    let url = format!(
        "http://{ip}:{port}/hello",
        ip = streamer_ip,
        port = STREAMER_PORT
    );

    let response = reqwest::get(&url).await?.json::<StartResponse>().await?;
    info!("Hello response: {:?}", response);

    Ok(response.ip)
}

pub async fn generate_sdp(streamer_ip: &str, yuv444: bool) -> Result<String> {
    let client = reqwest::Client::new();

    let url = format!(
        "http://{ip}:{port}/generate_sdp",
        ip = streamer_ip,
        port = STREAMER_PORT
    );

    let body = json!({
        "yuv444": yuv444,
    });

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .json::<GenerateSdpResponse>()
        .await?;
    info!("Generate SDP response: {:?}", response);

    Ok(response.params)
}

pub async fn start_stream(
    streamer_ip: &str,
    local_ip: &str,
    video_port: u16,
    yuv444: bool,
    inputs: bool,
) -> Result<()> {
    let client = reqwest::Client::new();

    let url = format!(
        "http://{ip}:{port}/start",
        ip = streamer_ip,
        port = STREAMER_PORT
    );

    let body = json!({
        "ip": local_ip,
        "port": video_port,
        "yuv444": yuv444,
        "inputs": inputs,
    });

    let response = client.post(&url).json(&body).send().await?;
    info!("Start response: {:?}", response);

    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(Error::StreamStartRejected)
    }
}

pub async fn stop_stream(streamer_ip: &str) -> Result<()> {
    info!("Request stop stream to {}", streamer_ip);

    let url = format!(
        "http://{ip}:{port}/stop",
        ip = streamer_ip,
        port = STREAMER_PORT
    );

    let client = reqwest::Client::new();

    let response = client.post(&url).send().await?;

    info!("Stop response: {:?}", response);

    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(Error::StreamStopRejected)
    }
}
