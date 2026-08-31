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

use kyctl_common::session::LoginResponse;
use kyctl_common::set_bitrate::SetBitrate;

use crate::error::Result;
use crate::http::HttpClient;
use crate::{AuthCredentials, Capabilities, Display, Error};

pub(crate) async fn capabilities(
    client: &HttpClient,
) -> Result<(Capabilities, Option<SocketAddr>)> {
    let url = format!("{base_url}/capabilities", base_url = client.base_url());

    let response = client
        .get(&url)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("GET failed: {err}"))?;

    let peer_addr = client.peer_addr_from_response(&response);

    Ok((response.json::<_>().await?, peer_addr))
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

    let response = request.send().await.inspect_err(|e| {
        if e.is_timeout() {
            warn!("POST /session/login has timed out");
        }
    })?;

    if response.status() == StatusCode::UNAUTHORIZED
        && let Some(value) = response.headers().get(reqwest::header::WWW_AUTHENTICATE)
        && let Ok(value) = value.to_str()
        && let Some(challenge) = parse_oidc_challenge(value)
    {
        return Ok(LoginResult::OidcRequired(challenge));
    }

    let response = response
        .error_for_status()
        .inspect_err(|err| error!("POST failed: {err}"))?;

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

pub(crate) enum RenewResult {
    Renewed,
    SessionGone,
}

pub(crate) async fn renew_session(
    client: &HttpClient,
    credentials: &AuthCredentials,
) -> Result<RenewResult> {
    let base_url = client.base_url();
    let request = match credentials {
        AuthCredentials::Basic { .. } => {
            warn!("Basic credentials cannot be renewed (no expiration)");
            return Err(Error::InvalidState);
        }
        AuthCredentials::Jwt { token } => {
            let url = format!("{base_url}/session/renew");
            client.post(&url).bearer_auth(token)
        }
        AuthCredentials::Oidc { token } => {
            let url = format!("{base_url}/session/renew?auth=oidc");
            client.post(&url).bearer_auth(token)
        }
    };

    let response = request.send().await.inspect_err(|e| {
        if e.is_timeout() {
            warn!("POST /session/renew has timed out");
        }
    })?;

    match response.error_for_status() {
        Ok(_) => Ok(RenewResult::Renewed),
        Err(e) => match e.status() {
            Some(StatusCode::UNAUTHORIZED | StatusCode::NOT_FOUND) => Ok(RenewResult::SessionGone),
            status => {
                status.inspect(|status| warn!("POST /session/renew failed: {status}"));
                Err(e.into())
            }
        },
    }
}

pub(crate) async fn enumerate_displays(client: &HttpClient) -> Result<Vec<Display>> {
    let url = format!(
        "{base_url}/enumerate_displays",
        base_url = client.base_url()
    );

    let response = client
        .get(&url)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("GET failed: {err}"))?;

    Ok(response.json().await?)
}

pub(crate) async fn stop_streaming(client: &HttpClient) -> Result<()> {
    let url = format!("{base_url}/stop_streaming", base_url = client.base_url());

    client
        .post(&url)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("POST failed: {err}"))?;

    Ok(())
}

pub(crate) async fn logout(client: &HttpClient) -> Result<()> {
    let url = format!("{base_url}/session/logout", base_url = client.base_url());

    client
        .post(&url)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("POST failed: {err}"))?;

    Ok(())
}

pub(crate) async fn set_bitrate(http_client: &HttpClient, bitrate: u32) -> Result<()> {
    let url = format!("{base_url}/set_bitrate", base_url = http_client.base_url());

    let body = SetBitrate { bitrate };

    http_client
        .post(&url)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .inspect_err(|err| warn!("POST failed: {err}"))?;

    Ok(())
}
