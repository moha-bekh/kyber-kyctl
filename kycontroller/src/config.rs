// Project Kyber: config.rs
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

use std::fs;

use log::{error, info};
use serde::Deserialize;

use crate::auth;

use super::utils;

const LISTENING_PORT: u16 = 8080;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    port: Option<u16>,
    dataplane_port: Option<u16>,
    webtransport_gen_certificate: Option<bool>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    tray: Option<bool>,
    #[serde(default)]
    auth: auth::Config,
    #[serde(default)]
    security: crate::security::Config,
}

impl Config {
    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or(LISTENING_PORT)
    }

    pub(crate) fn dataplane_port(&self) -> Option<u16> {
        self.dataplane_port
    }

    pub(crate) fn webtransport_gen_certificate(&self) -> bool {
        self.webtransport_gen_certificate.unwrap_or(true)
    }

    pub(crate) fn tray_enabled(&self) -> bool {
        self.tray.unwrap_or(true)
    }

    pub(crate) fn auth(&self) -> &auth::Config {
        &self.auth
    }

    pub(crate) fn security(&self) -> &crate::security::Config {
        &self.security
    }

    pub(crate) fn tls_cert(&self) -> Option<&str> {
        self.tls_cert.as_deref()
    }

    pub(crate) fn tls_key(&self) -> Option<&str> {
        self.tls_key.as_deref()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("text isn't valid UTF8")]
    InvalidUTF8,
    #[error(transparent)]
    InvalidTOML(#[from] toml::de::Error),
}

pub(crate) fn load() -> Result<Config, ParseError> {
    let path = utils::get_resource_path("kyber_config.toml");

    let Ok(content) = fs::read(&path) else {
        info!("Use default configuration");
        return Ok(Config::default());
    };

    info!("Using configuration file {path:?}");

    let content: toml::Table = str::from_utf8(&content)
        .map_err(|_| ParseError::InvalidUTF8)?
        .parse()?;

    let Some(controller_conf) = content.get("kycontroller") else {
        info!("No kycontroller section found. Use default configuration");
        return Ok(Config::default());
    };

    controller_conf
        .clone()
        .try_into()
        .map_err(ParseError::InvalidTOML)
}
