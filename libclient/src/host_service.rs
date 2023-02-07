use log::info;
use reqwest::{self, StatusCode};

use crate::error::{Error, Result};
use crate::http::HttpClient;

pub(crate) async fn stop_stream(client: &HttpClient) -> Result<()> {
    let url = format!("{base_url}/stop_session", base_url = client.base_url());

    info!("Request stop stream: {url}");
    let response = client.client().post(&url).send().await?;
    info!("Stop response: {:?}", response);

    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(Error::StreamStopRejected)
    }
}
