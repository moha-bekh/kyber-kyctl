// Project Kyber: config.rs
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

use std::fs;
use std::path::{Path, PathBuf};

use log::info;
use serde::Deserialize;

use crate::auth;

use super::utils;

const CONFIG_FILE: &str = "kyber_config.toml";
const LISTENING_PORT: u16 = 8080;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ListenMode {
    Ipv4,
    #[default]
    DualStack,
}

/// Concurrency model selected by the `multi_client` config flag.
///
/// `Multi` (default, `multi_client = true`): many concurrent clients share
/// one avservice per display via kydup. The RTP backend is rejected in this
/// mode (no avservice-sharing path).
///
/// `Mono` (`multi_client = false`): one concurrent client allowed at the
/// login gate; avservices are per-session and feed the client endpoint
/// directly via kycom, no kydup fan-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionMode {
    Mono,
    Multi,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    port: Option<u16>,
    listen_mode: Option<ListenMode>,
    dataplane_port: Option<u16>,
    webtransport_gen_certificate: Option<bool>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    tray: Option<bool>,
    #[serde(default)]
    auth: auth::Config,
    #[serde(default)]
    security: crate::security::Config,
    watchdog: Option<bool>,
    multi_client: Option<bool>,
    webclient: Option<String>,
}

impl Config {
    pub(crate) fn port(&self) -> u16 {
        self.port.unwrap_or(LISTENING_PORT)
    }

    pub(crate) fn listen_mode(&self) -> ListenMode {
        self.listen_mode.unwrap_or_default()
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

    pub(crate) fn watchdog(&self) -> bool {
        self.watchdog.unwrap_or(true)
    }

    pub(crate) fn tls_cert(&self) -> Option<&str> {
        self.tls_cert.as_deref()
    }

    pub(crate) fn tls_key(&self) -> Option<&str> {
        self.tls_key.as_deref()
    }

    pub(crate) fn session_mode(&self) -> SessionMode {
        match self.multi_client.unwrap_or(true) {
            true => SessionMode::Multi,
            false => SessionMode::Mono,
        }
    }

    pub(crate) fn webclient(&self) -> Option<&str> {
        self.webclient.as_deref()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("cannot read configuration file {path:?}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("text isn't valid UTF8")]
    InvalidUTF8,
    #[error(transparent)]
    InvalidTOML(#[from] toml::de::Error),
}

/// The config file the controller reads: an explicit `--config`, else
/// `KYBER_CONFIG`, else one next to our own executable. `main` exports the
/// resolved path so every spawned service reads the same file.
pub(crate) fn config_path(override_path: Option<&Path>) -> PathBuf {
    match override_path {
        Some(p) => p.to_path_buf(),
        None => match std::env::var_os("KYBER_CONFIG") {
            Some(p) => PathBuf::from(p),
            None => utils::get_resource_path(CONFIG_FILE),
        },
    }
}

pub(crate) fn parse(override_path: Option<&Path>) -> Result<Option<toml::Table>, ParseError> {
    // An explicit pointer must resolve to a readable file: --config, or
    // KYBER_CONFIG when set. With neither, a missing executable-relative
    // default falls back to the built-in defaults.
    let required = override_path.is_some() || std::env::var_os("KYBER_CONFIG").is_some();
    let path = config_path(override_path);
    let content = match fs::read(&path) {
        Ok(content) => content,
        Err(source) if required => return Err(ParseError::Read { path, source }),
        Err(_) => return Ok(None),
    };

    let table: toml::Table = str::from_utf8(&content)
        .map_err(|_| ParseError::InvalidUTF8)?
        .parse()?;

    Ok(Some(table))
}

pub(crate) fn from_table(table: &toml::Table) -> Result<Config, ParseError> {
    let Some(controller_conf) = table.get("kycontroller") else {
        info!("No kycontroller section found. Use default configuration");
        return Ok(Config::default());
    };

    controller_conf
        .clone()
        .try_into()
        .map_err(ParseError::InvalidTOML)
}
