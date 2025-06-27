use std::time::Duration;

use base64::Engine;
use log::{error, warn};
use reqwest::{self, header, StatusCode};
use serde::{Deserialize, Serialize};

#[cfg(target_family = "wasm")]
use kywasmtime::timeout;
#[cfg(not(target_family = "wasm"))]
use tokio::time::timeout;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{AuthCredentials, Capabilities, Display};

#[derive(Debug, Serialize)]
pub(crate) struct SetBitrateRequest {
    pub(crate) bitrate: u32,
}

pub(crate) async fn capabilities(client: &HttpClient) -> Result<Capabilities> {
    const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

    let url = format!("{base_url}/capabilities", base_url = client.base_url());

    let ret = timeout(HTTP_TIMEOUT, client.get(&url).send()).await;
    let response = match ret {
        Err(_) => {
            warn!("GET /capabilities has timed out");
            return Err(Error::HttpQueryTimeout);
        }
        Ok(ret) => ret?,
    };

    if response.status() != StatusCode::OK {
        warn!("GET /capabilities failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(response.json::<_>().await?)
}

#[derive(Deserialize)]
pub(crate) struct LoginResponse {
    pub(crate) uid: String,
    pub(crate) websocket: String,
    // TODO(alaric): Dataplane token.
}

pub(crate) async fn login(
    client: &HttpClient,
    credentials: &AuthCredentials,
) -> Result<LoginResponse> {
    let url = format!("{base_url}/session/login", base_url = client.base_url());
    let authorization = match credentials {
        AuthCredentials::Basic { username } => {
            let encoded = base64::prelude::BASE64_STANDARD.encode(format!("{username}:"));
            format!("Basic {encoded}")
        }
        AuthCredentials::Jwt { token } => format!("Bearer {token}"),
    };

    let response = client
        .post(url)
        .header(header::AUTHORIZATION, authorization)
        .send()
        .await?;
    if response.status() != StatusCode::OK {
        error!("GET /session/login failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(response.json().await?)
}

pub(crate) async fn enumerate_displays(client: &HttpClient) -> Result<Vec<Display>> {
    let url = format!(
        "{base_url}/enumerate_displays",
        base_url = client.base_url()
    );

    let response = client.get(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("GET /enumerate_displays failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(response.json::<_>().await?)
}

pub(crate) async fn stop_streaming(client: &HttpClient) -> Result<()> {
    let url = format!("{base_url}/stop_streaming", base_url = client.base_url());

    let response = client.post(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("POST /stop_streaming failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(())
}

pub(crate) async fn logout(client: &HttpClient) -> Result<()> {
    let url = format!("{base_url}/session/logout", base_url = client.base_url());

    let response = client.post(&url).send().await?;
    if response.status() != StatusCode::OK {
        warn!("POST /session/logout failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(())
}

pub(crate) async fn set_bitrate(http_client: &HttpClient, bitrate: u32) -> Result<()> {
    let url = format!("{base_url}/set_bitrate", base_url = http_client.base_url());

    let body = SetBitrateRequest { bitrate };

    let response = http_client.post(&url).json(&body).send().await?;
    if response.status() != StatusCode::OK {
        warn!("POST /set_bitrate failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(())
}
