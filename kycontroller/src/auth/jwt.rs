// Project Kyber: jwt.rs
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

//! JWT (JSON Web Token) authentication backend.
//!
//! This module implements RFC 7519 JWT validation with support for HS256 (HMAC)
//! and RS256 (RSA) signing algorithms. It integrates with the `jsonwebtoken` crate
//! for cryptographic operations.
//!
//! ## Supported Algorithms
//!
//! Even though algorithm support is easily extendable, Kyber has only been tested with:
//! - **HS256**: HMAC with SHA-256 (symmetric key)
//! - **RS256**: RSA Signature with SHA-256 (asymmetric key)
//!
//! ## JWT Claims
//!
//! Kyber requires these standard JWT claims:
//!
//! - `sub` (subject): Username/user identifier
//! - `exp` (expiration): Unix timestamp when the token expires
//! - `aud` (audience): Must be "kyber"
//!
//! ## Configuration Examples
//!
//! ### RS256 as a PEM file
//!
//! ```toml
//! [kycontroller.auth.jwt]
//! algorithm = "RS256"
//! key = { file = "/etc/kyber/jwt-public-key.pem" }
//! ```
//!
//! ### HS256 as plain text
//!
//! ```toml
//! [kycontroller.auth.jwt]
//! algorithm = "HS256"
//! key = { plain = "your-secret-key-here" }
//! ```

use std::{fs::File, io::Read};

use crate::auth::Backend;

use super::{Error, Session};
use jsonwebtoken::{
    decode,
    errors::{Error as JWTError, ErrorKind},
    Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigKey {
    /// Key provided directly as a string.
    Plain(String),

    /// Path to a file containing the key.
    File(std::path::PathBuf),
}

/// JWT backend configuration.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// JWT signing algorithm (HS256 or RS256).
    algorithm: Algorithm,
    /// Decoding/verification key configuration.
    key: ConfigKey,
}

impl Default for Config {
    /// Default configuration for development.
    ///
    /// Uses HS256 with a well-known development key. This is intentionally insecure
    /// and should only be used for local development/testing.
    fn default() -> Self {
        const DEFAULT_DEV_KEY: &str = "kyber-authentication-development-key";
        Self {
            algorithm: Algorithm::HS256,
            key: ConfigKey::Plain(DEFAULT_DEV_KEY.to_owned()),
        }
    }
}

/// JWT claims that Kyber expects in every token.
///
/// These are standard JWT claims as defined in RFC 7519 Section 4.1.
#[derive(Debug, Deserialize)]
struct Claims {
    /// Audience - identifies the intended recipient of the token.
    /// Kyber requires this to be "kyber" to prevent token reuse from other services.
    #[allow(unused)]
    aud: String,

    /// Expiration Time - Unix timestamp (seconds since epoch) when the token expires.
    /// Used to set the session expiration time.
    exp: i64,

    /// Subject - identifies the principal (user) that the token represents.
    /// This becomes the `username` in the session.
    sub: String,
}

pub struct Jwt {
    /// Cryptographic key for JWT signature verification
    key: DecodingKey,
    /// Validation rules (required claims, audience, expiration checks)
    validation: Validation,
}

impl Backend for Jwt {
    /// Validates a JWT token and extracts user session information.
    ///
    /// ## Validation Process
    ///
    /// 1. Decode the JWT header and payload
    /// 2. Verify the cryptographic signature
    /// 3. Validate required claims (`sub`, `exp`, `aud`)
    /// 4. Check expiration time (zero leeway)
    /// 5. Extract username from `sub` claim
    /// 6. Extract expiration from `exp` claim
    ///
    /// ## Special Handling for Expired Tokens
    ///
    /// When a token is expired, the `jsonwebtoken` crate doesn't expose the expiration
    /// timestamp in the error. To provide a helpful error message, we re-decode the token
    /// *without* expiration validation to extract the timestamp, then return an `Error::Expired`
    /// with the exact expiration time.
    ///
    /// # Parameters
    /// - `token`: The JWT token string
    ///
    /// # Returns
    /// - `Ok(Session)`: Valid token with extracted username and expiration
    /// - `Err(Error)`: Validation failed (expired, invalid signature, missing claims, etc.)
    fn authenticate(&self, token: &str) -> Result<Session, Error> {
        let jwt = decode::<Claims>(token, &self.key, &self.validation);

        // Special handling: Extract expiration timestamp from expired tokens
        // The jsonwebtoken library doesn't expose the exp claim in the error,
        // so we re-decode without expiration validation to get the timestamp.
        if let Err(e) = &jwt {
            if matches!(e.kind(), ErrorKind::ExpiredSignature) {
                let mut validation = self.validation.clone();
                validation.validate_exp = false;
                let expired = decode::<Claims>(token, &self.key, &validation)?;
                let at = OffsetDateTime::from_unix_timestamp(expired.claims.exp)
                    .or(Err(Error::InvalidCredentialsFormat))?;
                return Err(Error::Expired { at });
            }
        }

        let jwt = jwt?;
        let exp = OffsetDateTime::from_unix_timestamp(jwt.claims.exp)
            .or(Err(Error::InvalidCredentialsFormat))?;
        Ok(Session {
            username: jwt.claims.sub,
            expiration: Some(exp),
        })
    }
}

impl Jwt {
    /// Initializes the JWT backend.
    ///
    /// This must be called exactly once at application startup before any authentication
    /// requests are processed.
    ///
    /// # Panics
    /// - If called more than once (programming error)
    /// - If the key file cannot be read
    /// - If the RSA PEM format is invalid (RS256 only)
    pub fn new(config: &Config) -> Self {
        let key = match &config.key {
            ConfigKey::Plain(key) => decode_key(config.algorithm, key.as_bytes()),
            ConfigKey::File(path) => {
                let mut key = Vec::new();
                let mut file = File::open(path).expect("Invalid key path");
                file.read_to_end(&mut key)
                    .expect("Failed to read key content");
                // Trim leading and trailing whitespace (including newlines) as the jsonwebtoken
                // crate doesn't handle them.
                let key = key.as_slice().trim_ascii();
                decode_key(config.algorithm, key)
            }
        };

        let mut validation = Validation::new(config.algorithm);
        validation.set_required_spec_claims(&["exp", "sub", "aud"]);
        validation.set_audience(&["kyber"]);

        validation.leeway = 0;

        log::info!(
            "JWT backend initialized with algorithm {:?}",
            config.algorithm
        );

        Self { key, validation }
    }
}

/// Creates a decoding key from raw bytes based on the algorithm.
///
/// # Parameters
/// - `algorithm`: The JWT signing algorithm (determines key format)
/// - `key`: Raw key bytes
///
/// # Panics
/// - If an unsupported algorithm is provided
/// - If the RSA PEM format is invalid (RS256 only)
fn decode_key(algorithm: Algorithm, key: &[u8]) -> DecodingKey {
    match algorithm {
        Algorithm::HS256 => DecodingKey::from_secret(key),
        Algorithm::RS256 => DecodingKey::from_rsa_pem(key).expect("Invalid RSA PEM"),
        _ => panic!("Unsupported JWT algorithm (currently supported: HS256, RS256)"),
    }
}

impl From<JWTError> for Error {
    fn from(from: JWTError) -> Self {
        match from.into_kind() {
            ErrorKind::InvalidSignature => Error::InvalidSignature,
            ErrorKind::MissingRequiredClaim(claim) => Error::MissingField { field: claim },
            _ => Error::InvalidCredentialsFormat,
        }
    }
}
