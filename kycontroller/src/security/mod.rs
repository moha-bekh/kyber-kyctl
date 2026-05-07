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

use actix_web::cookie;
use log::warn;

pub mod cop;

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    trusted_origins: TrustedOrigins,
    same_site: bool,
}

impl Config {
    pub fn same_site(&self) -> cookie::SameSite {
        if self.same_site {
            cookie::SameSite::Strict
        } else {
            cookie::SameSite::None
        }
    }

    pub fn trusted_origins(&self) -> &TrustedOrigins {
        &self.trusted_origins
    }

    pub fn warn_if_insecure(&self) {
        match &self.trusted_origins {
            TrustedOrigins::All => warn!(
                "All cross-origin requests are accepted. \
               This is fine for development but should not be used in production."
            ),
            TrustedOrigins::Some(origins) if origins.is_empty() && !self.same_site => warn!(
                "SameSite=None cookie with no trusted origins configured. \
               Cross-origin requests will be rejected. \
               Add trusted_origins or set same_site = true in the config."
            ),
            _ => (),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            trusted_origins: Default::default(),
            same_site: true,
        }
    }
}

#[derive(Clone, Debug)]
pub enum TrustedOrigins {
    All,
    Some(Vec<String>),
}

impl TrustedOrigins {
    fn contains(&self, origin: &str) -> bool {
        match self {
            TrustedOrigins::All => true,
            TrustedOrigins::Some(origins) => origins.iter().any(|o| o == origin),
        }
    }
}

impl Default for TrustedOrigins {
    fn default() -> Self {
        Self::Some(vec![])
    }
}

impl<'de> serde::Deserialize<'de> for TrustedOrigins {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw {
            String(String),
            Array(Vec<String>),
        }

        match Raw::deserialize(deserializer)? {
            Raw::String(ref s) => match s.trim() {
                "*" => Ok(TrustedOrigins::All),
                other => Err(serde::de::Error::custom(format!(
                    "unknown value: {other}, expected \"*\" or an array"
                ))),
            },
            Raw::Array(v) => Ok(TrustedOrigins::Some(v)),
        }
    }
}

pub fn build_cors(origins: &TrustedOrigins) -> actix_cors::Cors {
    let cors = actix_cors::Cors::default()
        .allowed_methods(vec!["GET", "POST"])
        .supports_credentials()
        .allowed_header(actix_web::http::header::CONTENT_TYPE)
        .allowed_header(actix_web::http::header::AUTHORIZATION)
        .expose_headers(vec![actix_web::http::header::WWW_AUTHENTICATE]);

    match origins {
        TrustedOrigins::All => cors.allow_any_origin(),
        TrustedOrigins::Some(origins) => origins
            .iter()
            .fold(cors, |cors, origin| cors.allowed_origin(origin)),
    }
}
