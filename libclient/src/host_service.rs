use log::warn;
use reqwest::{self, StatusCode};

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{Capabilities, Display};

pub(crate) async fn capabilities(client: &HttpClient) -> Result<Capabilities> {
    let url = format!("{base_url}/capabilities", base_url = client.base_url());

    let response = client.client().get(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /capabilities failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(response.json::<_>().await?)
}

pub(crate) async fn enumerate_displays(client: &HttpClient) -> Result<Vec<Display>> {
    let url = format!(
        "{base_url}/enumerate_displays",
        base_url = client.base_url()
    );

    let response = client.client().get(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /enumerate_displays failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(response.json::<_>().await?)
}

pub(crate) async fn stop_stream(client: &HttpClient) -> Result<()> {
    let url = format!("{base_url}/stop_session", base_url = client.base_url());

    let response = client.client().post(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("POST /stop_session failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(())
}
