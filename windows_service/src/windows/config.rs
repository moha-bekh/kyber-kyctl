use std::fs;

use log::{error, info};
use serde::Deserialize;

use kidd_client::MonitorMode;

use super::utils;
use super::{Error, Result};

#[derive(Clone, Copy, Debug, Deserialize)]
#[allow(non_camel_case_types)]
pub(crate) enum KiddPlugStrategy {
    when_no_monitor,
    always,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct KiddMonitorMode {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) vsync: u32,
}

#[allow(clippy::from_over_into)]
impl Into<kidd_client::MonitorMode> for KiddMonitorMode {
    fn into(self) -> kidd_client::MonitorMode {
        kidd_client::MonitorMode {
            width: self.width,
            height: self.height,
            vsync: self.vsync,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct KiddConfig {
    pub(crate) enabled: Option<bool>,
    pub(crate) plug_strategy: Option<KiddPlugStrategy>,
    pub(crate) extra_modes: Option<Vec<KiddMonitorMode>>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    pub(crate) kidd: Option<KiddConfig>,
}

impl Config {
    pub(crate) fn kidd_extra_modes(&self) -> Option<Vec<KiddMonitorMode>> {
        let Some(kidd_config) = &self.kidd else {
            return None;
        };

        let Some(extra_modes) = &kidd_config.extra_modes else {
            return None;
        };

        Some(extra_modes.clone())
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

    let Some(avserver_conf) = content.get("windows_service") else {
        info!("No windows_service section found. Use default configuration");
        return Ok(Config::default());
    };

    avserver_conf
        .clone()
        .try_into()
        .map_err(|err| Error::ConfigLoadFail { reason: err })
}

pub(crate) const DEFAULT_MONITOR_MODES: &[MonitorMode] = &[
    MonitorMode {
        width: 1920,
        height: 1080,
        vsync: 60,
    },
    MonitorMode {
        width: 3840,
        height: 2160,
        vsync: 60,
    },
    MonitorMode {
        width: 3024,
        height: 1964,
        vsync: 60,
    },
    MonitorMode {
        width: 3072,
        height: 1920,
        vsync: 60,
    },
    MonitorMode {
        width: 2880,
        height: 1800,
        vsync: 60,
    },
    MonitorMode {
        width: 2560,
        height: 1664,
        vsync: 60,
    },
    MonitorMode {
        width: 2560,
        height: 1600,
        vsync: 60,
    },
    MonitorMode {
        width: 2560,
        height: 1440,
        vsync: 60,
    },
    MonitorMode {
        width: 2304,
        height: 1440,
        vsync: 60,
    },
    MonitorMode {
        width: 1920,
        height: 1200,
        vsync: 60,
    },
    MonitorMode {
        width: 1920,
        height: 1080,
        vsync: 144,
    },
    MonitorMode {
        width: 1024,
        height: 768,
        vsync: 60,
    },
    MonitorMode {
        width: 800,
        height: 600,
        vsync: 60,
    },
];

pub(crate) const DEFAULT_MONITOR_MODE_IDX: u32 = 0;
