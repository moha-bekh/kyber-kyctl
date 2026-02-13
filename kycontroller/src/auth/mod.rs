// Project Kyber: mod.rs
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

//! Authentication backend system.
//!
//! This module implements a simple architecture that allow multiple authentication methods through
//! Cargo feature flags. This design allows Kyber to integrate with various authentication
//! infrastructures while maintaining a consistent internal session management system.
//!
//! ## Supported Backends
//!
//! - **Basic Auth** (`auth-basic` feature, enabled by default): Passwordless authentication
//!   for development. Accepts any username without validation. **Must be disabled in production.**
//!
//! - **JWT** (`auth-jwt` feature): JSON Web Token authentication supporting HS256 and RS256
//!   algorithms with configurable keys and automatic expiration handling.
//!
//! ## Authentication Flow
//!
//! ```text
//! Client → Authorization header → from_authorization_header()
//!   → Extract (type, credentials)
//!   → authenticate(type, credentials)
//!   → Backend validation
//!   → Session { username, expiration }
//!   → Identity::login() creates cookie
//! ```

use actix_web::http;
use time::OffsetDateTime;

/// Authentication method extracted from the Authorization header.
#[derive(Debug)]
pub enum AuthenticationType {
    #[cfg(feature = "auth-basic")]
    Basic,
    #[cfg(feature = "auth-jwt")]
    Jwt,
}

/// Parsed HTTP `Authorization` header.
pub struct AuthorizationHeader<'hdr> {
    pub kind: AuthenticationType,
    pub creds: &'hdr str,
}

impl<'hdr> AuthorizationHeader<'hdr> {
    /// Extracts method and credentials from an `Authorization` header value.
    pub fn from_header(header: &'hdr http::header::HeaderValue) -> Option<Self> {
        let Ok(header) = header.to_str() else {
            return None;
        };

        #[cfg(feature = "auth-jwt")]
        if let Some(creds) = header.strip_prefix("Bearer").map(str::trim) {
            return Some(AuthorizationHeader {
                kind: AuthenticationType::Jwt,
                creds,
            });
        }

        #[cfg(feature = "auth-basic")]
        if let Some(creds) = header.strip_prefix("Basic").map(str::trim) {
            return Some(AuthorizationHeader {
                kind: AuthenticationType::Basic,
                creds,
            });
        }

        None
    }
}

/// Common Kyber authenticated session.
pub struct Session {
    /// User identifier
    pub username: String,
    /// Session expiration time
    pub expiration: Option<OffsetDateTime>,
}

/// Routes authentication to the appropriate backend based on type.
///
/// This is the main entry point for credential validation. It dispatches to
/// the backend module corresponding to the authentication type.
///
/// # Parameters
/// - `auth_type`: The authentication method (Basic, JWT, etc.)
/// - `credentials`: The extracted credentials (username, JWT token, etc.)
///
/// # Returns
/// - `Ok(Session)`: Valid credentials with username and optional expiration
/// - `Err(Error)`: Authentication failed (invalid credentials, expired token, etc.)
pub fn authenticate(header: &AuthorizationHeader) -> Result<Session, Error> {
    match header.kind {
        #[cfg(feature = "auth-basic")]
        AuthenticationType::Basic => basic::authenticate(header.creds),
        #[cfg(feature = "auth-jwt")]
        AuthenticationType::Jwt => jwt::authenticate(header.creds),
    }
}

/// Configuration for authentication backends loaded from TOML.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
pub struct Config {
    #[cfg(feature = "auth-jwt")]
    jwt: jwt::Config,
}

/// Initializes all enabled authentication backends.
///
/// This must be called once at application startup, before any authentication
/// requests are processed. Each backend performs one-time initialization such as
/// loading cryptographic keys.
pub fn init(config: &Config) {
    #[cfg(feature = "auth-jwt")]
    jwt::init(&config.jwt);
}

/// Authentication errors returned by backend validation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Credential has expired
    #[error("Expired at {at}")]
    Expired { at: OffsetDateTime },

    /// Cryptographic signature validation failed
    #[error("Invalid signature")]
    InvalidSignature,

    /// Credentials format is invalid or cannot be decoded
    #[error("Invalid credentials format")]
    InvalidCredentialsFormat,

    /// Required credential field is missing
    #[error("Missing mandatory field: {field}")]
    MissingField { field: String },
}

// JWT authentication backend (conditionally compiled)
#[cfg(feature = "auth-jwt")]
mod jwt;

#[cfg(feature = "auth-basic")]
mod basic;
