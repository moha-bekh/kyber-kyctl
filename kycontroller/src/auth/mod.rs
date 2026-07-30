// Project Kyber: mod.rs
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

//! Authentication backend system.
//!
//! This module implements a simple architecture that allow multiple authentication methods through
//! Cargo feature flags. This design allows Kyber to integrate with various authentication
//! infrastructures while maintaining a consistent internal session management system.
//!
//! ## Supported Backends
//!
//! - **Basic Auth** (`auth-basic` feature, enabled by default): For development. Credentials
//!   transit as base64-encoded plain text and password strength depends entirely on user choices.
//!   **Must be disabled in production.**
//!
//! - **JWT** (`auth-jwt` feature): JSON Web Token authentication supporting HS256 and RS256
//!   algorithms with configurable keys and automatic expiration handling.
//!
//! - **OIDC** (`auth-oidc` feature): OpenID Connect authentication with JWKS-based signature
//!   verification. Provider metadata and the key set are fetched on each authentication
//!   request, so key rotation is handled automatically.

use std::sync::OnceLock;

use actix_web::http;
use time::OffsetDateTime;

use crate::auth::registry::BackendRegistry;

/// Supported authentication methods.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    #[cfg(feature = "auth-basic")]
    Basic,
    #[cfg(feature = "auth-jwt")]
    Jwt,
    #[cfg(feature = "auth-oidc")]
    Oidc,
}

pub trait Backend {
    /// Validates credentials and returns a session on success.
    fn authenticate(&self, credentials: &str) -> Result<Session, Error>;
}

/// Parsed HTTP `Authorization` header.
pub struct AuthorizationHeader<'hdr> {
    pub kind: Method,
    pub creds: &'hdr str,
}

impl<'hdr> AuthorizationHeader<'hdr> {
    /// Extracts method and credentials from an `Authorization` header value.
    ///
    /// `auth_hint` disambiguates Bearer tokens: when set to `"oidc"`, a Bearer
    /// token is routed to the OIDC backend instead of the JWT backend.
    pub fn from_header(
        header: &'hdr http::header::HeaderValue,
        auth_hint: Option<&str>,
    ) -> Option<Self> {
        let Ok(header) = header.to_str() else {
            return None;
        };

        #[allow(unused)]
        if let Some(creds) = header.strip_prefix("Bearer").map(str::trim) {
            let kind = match auth_hint {
                #[cfg(feature = "auth-oidc")]
                Some("oidc") => Method::Oidc,
                _ => {
                    #[cfg(not(feature = "auth-jwt"))]
                    return None;

                    #[cfg(feature = "auth-jwt")]
                    Method::Jwt
                }
            };
            return Some(AuthorizationHeader { kind, creds });
        }

        #[cfg(feature = "auth-basic")]
        if let Some(creds) = header.strip_prefix("Basic").map(str::trim) {
            return Some(AuthorizationHeader {
                kind: Method::Basic,
                creds,
            });
        }

        None
    }
}

/// Common Kyber authenticated session.
#[derive(Debug)]
pub struct Session {
    /// User identifier
    pub username: String,
    /// Session expiration time
    pub expiration: Option<OffsetDateTime>,
}

/// Configuration for authentication backends loaded from TOML.
#[derive(Debug, serde::Deserialize, Default)]
#[serde(default)]
pub struct Config {
    #[cfg(feature = "auth-basic")]
    basic: OptionalBackendConfig<basic::Config>,
    #[cfg(feature = "auth-jwt")]
    jwt: OptionalBackendConfig<jwt::Config>,
    #[cfg(feature = "auth-oidc")]
    oidc: OptionalBackendConfig<oidc::Config>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(default)]
struct OptionalBackendConfig<T> {
    enabled: bool,
    #[serde(flatten)]
    inner: T,
}

impl<T: Default> Default for OptionalBackendConfig<T> {
    fn default() -> Self {
        Self {
            enabled: true,
            inner: Default::default(),
        }
    }
}

/// Initializes all enabled authentication backends.
///
/// This must be called once at application startup, before any authentication
/// requests are processed.
pub async fn init(config: &Config) -> Result<(), String> {
    let registry = BackendRegistry::new(config).await?;
    BACKEND_REGISTRY
        .set(registry)
        .or(Err(()))
        .expect("Cannot call init() twice");
    Ok(())
}

/// Returns the initialized backend registry.
///
/// Panics if called before [`init`].
pub fn registry<'a>() -> &'a BackendRegistry {
    BACKEND_REGISTRY.get().expect("Backends not initialized")
}

static BACKEND_REGISTRY: OnceLock<registry::BackendRegistry> = OnceLock::new();

/// Authentication errors returned by backend validation.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
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

    /// Identified user is not allowed in
    #[error("User \"{identity}\" is not authorized")]
    NotAuthorized { identity: String },

    /// OIDC provider is temporarily unreachable
    #[error("OIDC provider unavailable: {reason}")]
    OidcUnavailable { reason: String },
}

mod registry;

// JWT authentication backend (conditionally compiled)
#[cfg(feature = "auth-jwt")]
mod jwt;

#[cfg(feature = "auth-basic")]
mod basic;

#[cfg(feature = "auth-oidc")]
pub(crate) mod oidc;
