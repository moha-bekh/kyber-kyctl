use std::fs;

use log::{error, info};
use serde::Deserialize;

use crate::{auth, session};

use super::utils;
use super::{Error, Result};

const LISTENING_PORT: u16 = 8080;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    port: Option<u16>,
    dataplane_port: Option<u16>,
    webtransport_gen_certificate: Option<bool>,
    tls_cert: Option<String>,
    tls_key: Option<String>,
    #[serde(default)]
    auth: auth::Config,
    #[serde(default)]
    session: session::Config,
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

    pub(crate) fn auth(&self) -> &auth::Config {
        &self.auth
    }

    pub(crate) fn session(&self) -> &session::Config {
        &self.session
    }

    pub(crate) fn tls_cert(&self) -> Option<&str> {
        self.tls_cert.as_deref()
    }

    pub(crate) fn tls_key(&self) -> Option<&str> {
        self.tls_key.as_deref()
    }
}

pub(crate) fn load() -> Result<Config> {
    let path = utils::get_resource_path("kyber_config.toml");

    let Ok(content) = fs::read(&path) else {
        info!("Use default configuration");
        return Ok(Config::default());
    };

    info!("Using configuration file {path:?}");
    let content = String::from_utf8(content).map_err(|err| {
        error!("Configuration file isn't a valid UTF8 file: {err:?}");
        Error::ConfigParseFail
    })?;

    let content: toml::Table = content.parse().map_err(|err| {
        error!("Configuration file parsing failed: {err:?}");
        Error::ConfigParseFail
    })?;

    let Some(controller_conf) = content.get("controller") else {
        info!("No controller section found. Use default configuration");
        return Ok(Config::default());
    };

    controller_conf
        .clone()
        .try_into()
        .map_err(|err| Error::ConfigLoadFail { reason: err })
}
