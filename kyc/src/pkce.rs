// Project Kyber: pkce.rs
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

//! PKCE Authorization Code Flow with loopback redirect.

use std::time::Duration;

use log::info;
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest::async_http_client,
    AuthorizationCode, ClientId, CsrfToken, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{Error, OidcTokens};

const IDP_TIMEOUT: Duration = Duration::from_secs(30);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// A pending PKCE Authorization Code Flow.
///
/// Created by [`PkceFlow::start`]. The caller must open [`auth_url`](Self::auth_url)
/// in a browser, then call [`wait_for_tokens`](Self::wait_for_tokens) to complete
/// the exchange.
pub struct PkceFlow {
    auth_url: String,
    listener: tokio::net::TcpListener,
    client: CoreClient,
    csrf_token: CsrfToken,
    nonce: Nonce,
    pkce_verifier: PkceCodeVerifier,
}

impl PkceFlow {
    /// Starts a new PKCE flow: discovers the provider, binds a loopback
    /// listener, and builds the authorization URL.
    ///
    /// The caller must open [`auth_url()`](Self::auth_url) in a browser to
    /// continue the flow.
    pub async fn start(issuer: &str, client_id: &str) -> Result<Self, Error> {
        let (provider_metadata, base_client) = discover(issuer, client_id).await?;

        let offline_access_scope = Scope::new("offline_access".to_owned());
        let supports_offline = provider_metadata
            .scopes_supported()
            .map(|scopes| scopes.contains(&offline_access_scope))
            .unwrap_or(false);

        // RFC 8252 section 7.3 requires 127.0.0.1 (not ::1) for loopback redirect URIs.
        // The redirect URI registered with the IdP must match exactly what the browser
        // follows, so switching to IPv6 would require IdP-side changes and is non-standard.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let redirect_uri = RedirectUrl::new(format!("http://127.0.0.1:{port}"))
            .map_err(|e| Error::Discovery(e.to_string()))?;

        let client = base_client.set_redirect_uri(redirect_uri);

        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let mut auth_request = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("email".to_owned()));

        if supports_offline {
            auth_request = auth_request.add_scope(offline_access_scope);
        }

        let (auth_url, csrf_token, nonce) = auth_request.set_pkce_challenge(pkce_challenge).url();

        Ok(Self {
            auth_url: auth_url.to_string(),
            listener,
            client,
            csrf_token,
            nonce,
            pkce_verifier,
        })
    }

    /// The authorization URL the user must visit in a browser.
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }

    /// Waits for the IdP redirect callback, validates CSRF state, exchanges
    /// the authorization code for tokens, and validates the nonce.
    ///
    /// Times out after 5 minutes if no callback is received.
    pub async fn wait_for_tokens(self) -> Result<OidcTokens, Error> {
        // Loop to skip spurious TCP connections (browser pre-connect, favicon requests)
        // that arrive before the real IdP redirect. The outer CALLBACK_TIMEOUT bounds
        // the total wait regardless of how many spurious connections arrive.
        let (code, state, mut response_stream) = loop {
            let (mut stream, _) = tokio::time::timeout(CALLBACK_TIMEOUT, self.listener.accept())
                .await
                .map_err(|_| Error::Callback("browser authentication timed out".to_owned()))??;

            match tokio::time::timeout(Duration::from_secs(5), read_callback(&mut stream)).await {
                Ok(Ok((code, state))) => break (code, state, stream),
                _ => {
                    stream
                        .write_all(b"HTTP/1.1 404 Not Found\r\n\r\n")
                        .await
                        .ok();
                }
            }
        };

        if state != *self.csrf_token.secret() {
            response_stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\n\r\n\
                      <html><body>Authentication failed: invalid state.</body></html>",
                )
                .await
                .ok();
            return Err(Error::Validation("CSRF state mismatch".to_owned()));
        }

        response_stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
                  <html><body>Authentication complete. You may close this tab.</body></html>",
            )
            .await
            .ok();

        let token_response = tokio::time::timeout(
            IDP_TIMEOUT,
            self.client
                .exchange_code(AuthorizationCode::new(code))
                .set_pkce_verifier(self.pkce_verifier)
                .request_async(async_http_client),
        )
        .await
        .map_err(|_| Error::TokenExchange("timed out".to_owned()))?
        .map_err(|e| Error::TokenExchange(format!("{e:?}")))?;

        if let Some(id_token) = token_response.id_token() {
            id_token
                .claims(&self.client.id_token_verifier(), &self.nonce)
                .map_err(|e| Error::Validation(format!("id_token verification failed: {e}")))?;
        }

        extract_tokens(&token_response)
    }
}

/// Silently refreshes OIDC tokens using a stored refresh token.
pub async fn refresh(
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<OidcTokens, Error> {
    let (_metadata, client) = discover(issuer, client_id).await?;

    let token_response = tokio::time::timeout(
        IDP_TIMEOUT,
        client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_owned()))
            .request_async(async_http_client),
    )
    .await
    .map_err(|_| Error::TokenExchange("timed out".to_owned()))?
    .map_err(|e| Error::TokenExchange(e.to_string()))?;

    extract_tokens(&token_response)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

async fn discover(
    issuer: &str,
    client_id: &str,
) -> Result<(CoreProviderMetadata, CoreClient), Error> {
    let issuer_url =
        IssuerUrl::new(issuer.to_owned()).map_err(|e| Error::Discovery(e.to_string()))?;

    let provider_metadata = tokio::time::timeout(
        IDP_TIMEOUT,
        CoreProviderMetadata::discover_async(issuer_url, async_http_client),
    )
    .await
    .map_err(|_| Error::Discovery("timed out".to_owned()))?
    .map_err(|e| Error::Discovery(e.to_string()))?;

    let client = CoreClient::from_provider_metadata(
        provider_metadata.clone(),
        ClientId::new(client_id.to_owned()),
        None,
    );

    Ok((provider_metadata, client))
}

fn extract_tokens(
    token_response: &openidconnect::core::CoreTokenResponse,
) -> Result<OidcTokens, Error> {
    let access_token = token_response.access_token().secret().to_owned();
    let id_token = token_response.id_token().map(|t| t.to_string());
    let refresh_token = token_response
        .refresh_token()
        .map(|t| t.secret().to_owned());
    let exp = id_token.as_deref().map(crate::jwt::parse_exp).transpose()?;

    info!(
        "OIDC refresh token: {}",
        if refresh_token.is_some() {
            "received"
        } else {
            "not provided by IdP"
        }
    );

    Ok(OidcTokens {
        access_token,
        id_token,
        refresh_token,
        exp,
    })
}

async fn read_callback(stream: &mut tokio::net::TcpStream) -> Result<(String, String), String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("read error: {e}"))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            return Err("HTTP request too large".to_owned());
        }
    }

    let request = std::str::from_utf8(&buf).map_err(|e| e.to_string())?;
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("missing request path")?;

    let parsed = url::Url::parse(&format!("http://127.0.0.1{path}")).map_err(|e| e.to_string())?;

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            "error_description" => error_description = Some(v.into_owned()),
            _ => {}
        }
    }

    if let Some(err) = error {
        let desc = error_description.unwrap_or(err);
        return Err(format!("IdP denied login: {desc}"));
    }

    Ok((
        code.ok_or("missing 'code' in callback")?,
        state.ok_or("missing 'state' in callback")?,
    ))
}
