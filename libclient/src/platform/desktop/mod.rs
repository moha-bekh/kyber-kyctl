// Project Kyber: mod.rs
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

use std::net::{SocketAddr, ToSocketAddrs};

use log::{error, info};

mod host_events;
pub(crate) mod http;
pub(crate) mod input_service;
mod mainloop_engine;
pub(crate) mod player;
pub(crate) mod tls;
pub(crate) mod window_handle;

pub(crate) use host_events::{WsClient, WsClientBuilder};
pub(crate) use input_service::{get_cursor_format, InputClient, InputError, InputPipeline};
pub(crate) use mainloop_engine::MainLoopEngine;
pub(crate) use window_handle::WindowHandle;

use crate::{AuthCredentials, Error, Result, TlsRootCert};

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub tls_host: Option<String>,
    pub tls_root_cert: Option<TlsRootCert>,
    pub credentials: AuthCredentials,
    /// Enable automatic reconnection on connection loss. Default: false.
    /// When enabled, the client will emit `Reconnecting`, `Reconnected`, and
    /// `ReconnectionFailed` events that the application must handle.
    pub auto_reconnection: bool,
}

#[derive(Clone, Debug)]
pub struct VideoPlayerConfig {
    pub window_handle: WindowHandle,
    pub software_decode: bool,
    pub video_buffer: u32, // ms
}

impl VideoPlayerConfig {
    pub fn print(&self) {
        info!("VideoPlayerConfig");
        info!("\twindow_handle: {:?}", self.window_handle);
        info!("\tsoftware_decode: {}", self.software_decode);
        info!("\tvideo_buffer: {}", self.video_buffer);
    }
}

#[derive(Clone, Debug)]
#[allow(unused)]
pub struct AudioPlayerConfig {
    pub audio_buffer: u32, // ms
}

#[derive(Clone, Debug)]
#[allow(unused)]
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) addr: std::net::SocketAddr,
    pub(crate) tls_host: Option<String>,
    pub(crate) tls_config: Option<tls::Config>,
    pub(crate) credentials: AuthCredentials,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        let addr = Self::resolve_streamer_host(&connect_config.host, connect_config.port)?;

        let tls_config = if let Some(tls_root_cert) = &connect_config.tls_root_cert {
            Some(tls::load_tls_config(tls_root_cert)?)
        } else {
            None
        };

        Ok(Self {
            host: connect_config.host,
            port: connect_config.port,
            addr,
            tls_host: connect_config.tls_host.clone(),
            tls_config,
            credentials: connect_config.credentials,
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
