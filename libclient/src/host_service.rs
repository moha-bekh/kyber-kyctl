use log::info;
use reqwest::{self, StatusCode};

use crate::error::{Error, Result};

pub(crate) const STREAMER_PORT: u16 = 8080;

pub async fn stop_stream(streamer_ip: &str) -> Result<()> {
    info!("Request stop stream to {}", streamer_ip);

    let url = format!(
        "http://{ip}:{port}/stop_session",
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
