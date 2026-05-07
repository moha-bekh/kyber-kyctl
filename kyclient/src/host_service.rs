// Project Kyber: host_service.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

use std::net::SocketAddr;

use log::{error, warn};
use reqwest::{self, StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{AuthCredentials, Capabilities, Display};

#[derive(Debug, Serialize)]
pub(crate) struct SetBitrateRequest {
    pub(crate) bitrate: u32,
}

pub(crate) async fn capabilities(
    client: &HttpClient,
) -> Result<(Capabilities, Option<SocketAddr>)> {
    let url = format!("{base_url}/capabilities", base_url = client.base_url());

    let response = client.get(&url).send().await?;

    if response.status() != StatusCode::OK {
        warn!("GET /capabilities failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    let peer_addr = client.peer_addr_from_response(&response);

    Ok((response.json::<_>().await?, peer_addr))
}

#[derive(Deserialize)]
pub(crate) struct LoginResponse {
    pub(crate) uid: String,
    pub(crate) websocket: String,
}

pub(crate) struct OidcChallenge {
    pub(crate) issuer: String,
    pub(crate) client_id: String,
}

pub(crate) enum LoginResult {
    Success(LoginResponse),
    OidcRequired(OidcChallenge),
}

pub(crate) async fn login(
    client: &HttpClient,
    credentials: &AuthCredentials,
) -> Result<LoginResult> {
    let base_url = client.base_url();
    let request = match credentials {
        AuthCredentials::Basic { username, password } => {
            let url = format!("{base_url}/session/login");
            client.post(&url).basic_auth(username, Some(password))
        }
        AuthCredentials::Jwt { token } => {
            let url = format!("{base_url}/session/login");
            client.post(&url).bearer_auth(token)
        }
        AuthCredentials::Oidc { token } => {
            let url = format!("{base_url}/session/login?auth=oidc");
            let request = client.post(&url);
            if token.is_empty() {
                request
            } else {
                request.bearer_auth(token)
            }
        }
    };

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            warn!("POST /session/login has timed out");
            return Err(Error::HttpQueryTimeout);
        }
        Err(e) => return Err(Error::HttpClient(e)),
    };

    if response.status() == StatusCode::UNAUTHORIZED {
        if let Some(challenge) = response
            .headers()
            .get(reqwest::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_oidc_challenge)
        {
            return Ok(LoginResult::OidcRequired(challenge));
        }
        error!("POST /session/login failed: 401 Unauthorized");
        return Err(Error::HttpQueryFailed { status: 401 });
    }

    if response.status() != StatusCode::OK {
        error!("POST /session/login failed: {}", response.status());
        return Err(Error::HttpQueryFailed {
            status: response.status().into(),
        });
    }

    Ok(LoginResult::Success(response.json().await?))
}

/// Parse the OIDC discovery challenge from a `WWW-Authenticate` header value.
///
/// The controller sends this header on 401 when OIDC is enabled and no token
/// was provided:
///
/// ```text
/// Bearer realm="oidc", issuer="https://idp.example.com", client_id="kyber-app"
/// ```
///
/// Returns `None` if the header is not an OIDC challenge (e.g. a plain Bearer
/// 401 from JWT auth) or if `issuer`/`client_id` are missing.
fn parse_oidc_challenge(header: &str) -> Option<OidcChallenge> {
    if !header.starts_with("Bearer") {
        return None;
    }

    // Distinguish our OIDC challenge from plain Bearer 401s (e.g. JWT auth).
    if !header.contains("realm=\"oidc\"") {
        return None;
    }

    Some(OidcChallenge {
        issuer: extract_quoted_param(header, "issuer")?,
        client_id: extract_quoted_param(header, "client_id")?,
    })
}

/// Extracts the value of `key="..."` from a Bearer challenge header.
/// Handles escaped quotes and commas inside the value (e.g. issuer URLs
/// with query strings containing commas, which RFC 3986 permits).
fn extract_quoted_param(header: &str, key: &str) -> Option<String> {
    let needle = format!("{}=\"", key);
    let start = header.find(&needle)? + needle.len();
    let mut value = String::new();
    let mut chars = header[start..].chars();
    loop {
        match chars.next()? {
            '\\' => {
                value.push(chars.next()?);
            }
            '"' => return Some(value),
            c => value.push(c),
        }
    }
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
