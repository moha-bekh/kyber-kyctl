use std::fs;

use log::{error, info};
use serde::Deserialize;

use super::utils;
use super::{Error, Result};

const LISTENING_PORT: u16 = 8080;

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    port: Option<u16>,
}

impl Config {
    pub(crate) fn port(&self) -> u16 {
        match self.port {
            Some(port) => port,
            None => LISTENING_PORT,
        }
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
