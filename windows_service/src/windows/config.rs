use std::fs;

use log::{error, info};
use serde::Deserialize;

use kidd_client::MonitorMode;

use super::utils;
use super::{Error, Result};

#[derive(Clone, Copy, Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KiddPlugStrategy {
    #[default]
    WhenNoMonitor,
    Always,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct KiddMonitorMode {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) vsync: u32,
}

impl From<&KiddMonitorMode> for kidd_client::MonitorMode {
    fn from(m: &KiddMonitorMode) -> Self {
        kidd_client::MonitorMode {
            width: m.width,
            height: m.height,
            vsync: m.vsync,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct KiddConfig {
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) plug_strategy: KiddPlugStrategy,
    #[serde(default)]
    pub(crate) extra_modes: Vec<KiddMonitorMode>,
    #[serde(default = "default_monitor_count")]
    pub(crate) monitor_count: u32,
}

fn default_enabled() -> bool {
    true
}

fn default_monitor_count() -> u32 {
    1
}

impl Default for KiddConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            plug_strategy: KiddPlugStrategy::default(),
            extra_modes: Vec::new(),
            monitor_count: default_monitor_count(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    pub(crate) kidd: KiddConfig,
}

#[derive(Debug, Default, Deserialize)]
struct Root {
    #[serde(default)]
    windows_service: Config,
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

    let config: Root = toml::from_str(&content).map_err(|err| {
        error!("Configuration file parsing failed: {err:?}");
        Error::ConfigParseFail
    })?;

    Ok(config.windows_service)
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
