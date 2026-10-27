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

use log::{info, warn};
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
    /// Read from the top-level [kyhub] section by `from_table`.
    #[cfg(feature = "kyhub")]
    #[serde(skip)]
    kyhub: Option<kyagent::Config>,
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

    #[cfg(feature = "kyhub")]
    #[inline]
    pub(crate) fn kyhub(&self) -> Option<&kyagent::Config> {
        self.kyhub.as_ref()
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

const SECTION: &str = "kycontroller";

/// Name of the section before the kyctl renaming. A file that still uses it
/// gets the defaults, so it is worth a warning rather than an info.
const LEGACY_SECTION: &str = "controller";

/// Keys of the section read by the logger (`log4ky`), not by `Config`.
const SHARED_KEYS: &[&str] = &["log_level", "log_levels"];

/// Deserialize the section, and return beside it the keys that nothing reads,
/// as dotted paths (`security.same_sit`). Serde skips them without an error,
/// so a misspelled key would otherwise leave its default in place unnoticed.
/// The keys of the flattened auth backends are not seen (see the tests).
fn load_section(section: &toml::Value) -> Result<(Config, Vec<String>), ParseError> {
    let mut unknown = Vec::new();
    let config = serde_ignored::deserialize(section.clone(), |path| {
        let key = dotted(&path);
        if !SHARED_KEYS.contains(&key.as_str()) {
            unknown.push(key);
        }
    })?;
    Ok((config, unknown))
}

/// `auth.basic.logins[0].username`: the path as written in the file. `Path`'s
/// own `Display` also prints the `Option` and newtype layers, as `?`, which
/// the file does not have.
fn dotted(path: &serde_ignored::Path) -> String {
    use serde_ignored::Path;
    match path {
        Path::Root => String::new(),
        Path::Seq { parent, index } => format!("{}[{index}]", dotted(parent)),
        Path::Map { parent, key } => match dotted(parent) {
            parent if parent.is_empty() => key.clone(),
            parent => format!("{parent}.{key}"),
        },
        Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => dotted(parent),
    }
}

pub(crate) fn from_table(table: &toml::Table) -> Result<Config, ParseError> {
    #[cfg_attr(not(feature = "kyhub"), allow(unused_mut))]
    let mut config = match table.get(SECTION) {
        Some(controller_conf) => {
            let (config, unknown) = load_section(controller_conf)?;
            for key in unknown {
                warn!("Unknown key `{key}` in [{SECTION}] is ignored");
            }
            config
        }
        None if table.contains_key(LEGACY_SECTION) => {
            warn!(
                "Section [{LEGACY_SECTION}] is ignored: it was renamed [{SECTION}], using defaults"
            );
            Config::default()
        }
        None => {
            info!("No kycontroller section found. Use default configuration");
            Config::default()
        }
    };

    #[cfg(feature = "kyhub")]
    {
        config.kyhub = kyagent::Config::from_toml(table)?;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unknown_keys(toml: &str) -> Vec<String> {
        let table: toml::Table = toml.parse().unwrap();
        load_section(&table[SECTION]).unwrap().1
    }

    #[test]
    fn unknown_keys_are_reported_with_their_path() {
        let keys = unknown_keys("[kycontroller]\nport = 9443\nwatchdg = false");
        assert_eq!(keys, ["watchdg"]);
    }

    #[test]
    fn keys_read_by_the_logger_are_not_unknown() {
        let keys = unknown_keys(
            "[kycontroller]\nlog_level = \"debug\"\nlog_levels = { kynet = \"trace\" }",
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    fn known_keys_are_not_unknown() {
        let keys = unknown_keys(
            "[kycontroller]\nport = 9443\nlisten_mode = \"ipv4\"\nwatchdog = false\n\
             multi_client = false\n\
             [kycontroller.security]\ntrusted_origins = []\nsame_site = true\n\
             [kycontroller.auth.basic]\nenabled = true\nlogins = []",
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    // The auth backends' settings are flattened beside `enabled`, and serde
    // hands a flattened struct the keys it does not know without telling
    // serde_ignored: a typo there is not reported. Pin that limit, so that a
    // fix of it shows up here.
    #[cfg(feature = "auth-jwt")]
    #[test]
    fn unknown_keys_in_flattened_backends_are_not_seen() {
        let keys = unknown_keys(
            "[kycontroller.auth.jwt]\nenabled = true\nalgoritm = \"RS256\"\n\
             key = { plain = \"k\" }",
        );
        assert!(keys.is_empty(), "{keys:?}");
    }

    #[test]
    fn legacy_section_falls_back_to_defaults() {
        let table: toml::Table = "[controller]\nport = 9443".parse().unwrap();
        let config = from_table(&table).unwrap();
        assert_eq!(config.port(), LISTENING_PORT);
    }
}
