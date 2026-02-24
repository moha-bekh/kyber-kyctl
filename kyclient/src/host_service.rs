// Project Kyber: host_service.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::time::Duration;

use log::{error, warn};
use reqwest::{self, StatusCode};
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
}

pub(crate) async fn login(
    client: &HttpClient,
    credentials: &AuthCredentials,
) -> Result<LoginResponse> {
    let url = format!("{base_url}/session/login", base_url = client.base_url());
    let request = client.post(url);

    let request = match credentials {
        AuthCredentials::Basic { username } => request.basic_auth(username, Option::<String>::None),
        AuthCredentials::Jwt { token } => request.bearer_auth(token),
    };

    let response = request.send().await?;
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
