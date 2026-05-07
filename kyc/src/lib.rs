// Project Kyber: lib.rs
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

//! OIDC Authorization Code Flow with PKCE for native apps.
//!
//! Provides a two-step interactive PKCE flow, silent token refresh,
//! cross-platform browser launch, and JWT helpers. The desktop client is the
//! first consumer; kycontroller will need it too to support OIDC login on the
//! controller side. AppAuth covers web and Android; kyc fills the same role for
//! Rust native targets.
//!
//! The library does **not** open a browser; the caller decides how and when
//! to present the authorization URL.
//!
//! # Usage
//!
//! ```no_run
//! # async fn example() -> Result<(), kyc::Error> {
//! // 1. Start the flow: returns the auth URL and a pending flow handle
//! let flow = kyc::PkceFlow::start("https://auth.example.com/realms/app", "my-client").await?;
//!
//! // 2. Open the URL however you want
//! kyc::browser::open_url(flow.auth_url()).ok();
//!
//! // 3. Wait for the redirect callback and exchange code for tokens
//! let tokens = flow.wait_for_tokens().await?;
//! log::info!("Authenticated, token expires at {:?}", tokens.exp);
//!
//! // Silent refresh later
//! if let Some(rt) = &tokens.refresh_token {
//!     let new_tokens = kyc::refresh_token("https://auth.example.com/realms/app", "my-client", rt).await?;
//! }
//! # Ok(())
//! # }
//! ```

pub mod browser;
mod error;
pub mod jwt;
mod pkce;

pub use error::Error;
pub use pkce::PkceFlow;

/// Tokens returned by the OIDC flow.
#[derive(Clone)]
pub struct OidcTokens {
    pub access_token: String,
    pub id_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Unix timestamp when the ID token expires (`None` if no id_token).
    pub exp: Option<u64>,
}

impl std::fmt::Debug for OidcTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcTokens")
            .field("access_token", &"[REDACTED]")
            .field("id_token", &self.id_token.as_ref().map(|_| "[REDACTED]"))
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("exp", &self.exp)
            .finish()
    }
}

/// Silently refreshes OIDC tokens using a stored refresh token.
///
/// Convenience wrapper around [`pkce::refresh`].
pub async fn refresh_token(
    issuer: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<OidcTokens, Error> {
    pkce::refresh(issuer, client_id, refresh_token).await
}
