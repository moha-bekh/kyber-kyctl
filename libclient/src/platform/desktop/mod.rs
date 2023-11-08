use std::net::{SocketAddr, ToSocketAddrs};

use log::error;

pub(crate) mod player;
pub(crate) mod tls;
pub(crate) mod window_handle;

pub(crate) use window_handle::WindowHandle;

use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub tls_host: String,
    pub tls_root_cert: String,
}

#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub window_handle: Option<WindowHandle>,
    pub software_decode: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) addr: std::net::SocketAddr,
    pub(crate) tls_host: String,
    pub(crate) tls_config: tls::Config,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        let addr = Self::resolve_streamer_host(&connect_config.host, connect_config.port)?;

        let tls_config = tls::load_tls_config(&connect_config.tls_root_cert)?;

        Ok(Self {
            host: connect_config.host,
            port: connect_config.port,
            addr,
            tls_host: connect_config.tls_host.clone(),
            tls_config,
        })
    }

    fn resolve_streamer_host(host: &str, port: u16) -> Result<SocketAddr> {
        let str_addr = format!("{host}:{port}");
        let mut addrs = str_addr.to_socket_addrs().map_err(|err| {
            error!("Fail to resolve host {host}: {err:?}");
            Error::FailToResolveAddress {
                addr: str_addr.clone(),
            }
        })?;

        let ipv4_addr = addrs.find(|addr| matches!(addr, SocketAddr::V4(_)));

        ipv4_addr.ok_or_else(|| {
            error!("Fail to find an IPv4 address for {host}");
            Error::FailToResolveAddress { addr: str_addr }
        })
    }
}
