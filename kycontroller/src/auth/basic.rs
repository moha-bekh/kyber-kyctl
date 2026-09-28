// Project Kyber: basic.rs
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

use super::{Backend, Error, Session};

use base64::{Engine, prelude::BASE64_STANDARD};
use serde::Deserialize;
use sha2::Digest;

/// HTTP Basic authentication backend (RFC 7617).
///
/// Validates credentials against a preset list defined in the controller configuration.
pub struct Basic {
    allowed_sessions: Vec<Credentials>,
}

impl Backend for Basic {
    fn authenticate(&self, credentials: &str) -> Result<Session, Error> {
        let credentials = Self::decode_credentials(credentials)?;

        if !self.allowed_sessions.contains(&credentials) {
            let identity = credentials.username.0;
            return Err(Error::NotAuthorized { identity });
        }

        Ok(Session {
            username: credentials.username.0,
            sub: None,
            expiration: None,
        })
    }
}

impl Basic {
    pub fn new(config: &Config) -> Self {
        log::info!(
            "Basic backend initialized with {} expected user identities",
            config.logins.len()
        );
        Self {
            allowed_sessions: config.logins.clone(),
        }
    }

    fn decode_credentials(credentials: &str) -> Result<Credentials, Error> {
        let plain_text = BASE64_STANDARD
            .decode(credentials)
            .or(Err(Error::InvalidCredentialsFormat))?;

        let mut parts = std::str::from_utf8(&plain_text)
            .or(Err(Error::InvalidCredentialsFormat))?
            .splitn(2, ':');

        let username = parts
            .next()
            .map(RFC7617String::from_str)
            .ok_or(Error::MissingField {
                field: "username".to_owned(),
            })?
            .or(Err(Error::InvalidCredentialsFormat))?;

        let password = parts
            .next()
            .map(RFC7617String::from_str)
            .ok_or(Error::MissingField {
                field: "password".to_owned(),
            })?
            .or(Err(Error::InvalidCredentialsFormat))?;

        Ok(Credentials {
            username,
            password: Sha256Sum(sha2::Sha256::digest(password.0).into()),
        })
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    logins: Vec<Credentials>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
struct Credentials {
    username: RFC7617String,
    password: Sha256Sum,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(transparent)]
struct Sha256Sum(#[serde(with = "hex::serde")] [u8; 32]);

/// A string validated per RFC 7617.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RFC7617String(String);

impl RFC7617String {
    fn from_str(raw: &str) -> Result<Self, &'static str> {
        if raw.is_empty() {
            Err("must not be empty")
        } else if raw.chars().any(|c| c.is_ascii_control()) {
            Err("must not contain CTL characters")
        } else {
            Ok(Self(raw.to_owned()))
        }
    }
}

struct RFC7617StringVisitor;
impl<'de> serde::de::Visitor<'de> for RFC7617StringVisitor {
    type Value = RFC7617String;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a valid RFC 7617 username")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if value.contains(':') {
            Err(E::custom("must not contain ':' characters"))
        } else {
            RFC7617String::from_str(value).map_err(E::custom)
        }
    }
}

impl<'de> Deserialize<'de> for RFC7617String {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(RFC7617StringVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, prelude::BASE64_STANDARD};

    fn encode(s: &str) -> String {
        BASE64_STANDARD.encode(s)
    }

    #[test]
    fn valid_simple_credentials() {
        let encoded = encode("user:pass");
        let result = Basic::decode_credentials(&encoded);
        assert!(result.is_ok());
        let creds = result.unwrap();
        assert_eq!(creds.username.0, "user");
    }

    #[test]
    fn valid_unicode() {
        let encoded = encode("用户:password");
        let result = Basic::decode_credentials(&encoded);
        assert!(result.is_ok());
        let creds = result.unwrap();
        assert_eq!(creds.username.0, "用户");

        let encoded = encode("user:密码");
        let result = Basic::decode_credentials(&encoded);
        assert!(result.is_ok());
    }

    #[test]
    fn valid_special_characters() {
        let encoded = encode("user@example.com:p@$$w0rd!#%");
        let result = Basic::decode_credentials(&encoded);
        assert!(result.is_ok());
        let creds = result.unwrap();
        assert_eq!(creds.username.0, "user@example.com");

        let encoded = encode("user name:pass word");
        let result = Basic::decode_credentials(&encoded);
        assert!(result.is_ok());
        let creds = result.unwrap();
        assert_eq!(creds.username.0, "user name");
    }

    #[test]
    fn invalid_base64() {
        let result = Basic::decode_credentials("not!valid@base64");
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));

        // Invalid base64 padding
        let result = Basic::decode_credentials("dXNlcjpwYXNz===");
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));
    }

    #[test]
    fn invalid_utf8() {
        let invalid_utf8 = BASE64_STANDARD.encode([0xFF, 0xFE, b':', b'p', b'a', b's', b's']);
        let result = Basic::decode_credentials(&invalid_utf8);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));
    }

    #[test]
    fn empty_fields() {
        let encoded = encode(":password");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));

        let encoded = encode("user:");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));

        let encoded = encode(":");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));
    }

    #[test]
    fn control_chars() {
        let encoded = encode("user\x00name:password");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));

        let encoded = encode("user:pass\x1Fword");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));

        let encoded = encode("user\nname:password");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));

        let encoded = encode("user:pass\tword");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));
    }

    #[test]
    fn colon_handling() {
        let encoded = encode("userpassword");
        let result = Basic::decode_credentials(&encoded);
        assert!(matches!(result, Err(Error::MissingField { field }) if field == "password"));

        // RFC7617 specifies that subsequents colon should be treated as part of the password.
        let encoded = encode("user:pass:word");
        let result = Basic::decode_credentials(&encoded);
        assert!(result.is_ok());
    }

    #[test]
    fn empty_input() {
        let result = Basic::decode_credentials("");
        assert!(matches!(result, Err(Error::InvalidCredentialsFormat)));
    }
}
