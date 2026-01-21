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

use std::sync::Arc;

use log::info;

pub(crate) mod fingerprint;
mod host_events;
pub(crate) mod http;
pub(crate) mod input_service;
pub(crate) mod known_hosts;
mod mainloop_engine;
pub(crate) mod message;
pub(crate) mod player;
pub(crate) mod tls;
pub(crate) mod window_handle;

pub(crate) use host_events::{WsClient, WsClientBuilder};
pub(crate) use input_service::{get_cursor_format, InputClient, InputError, InputPipeline};
pub(crate) use mainloop_engine::MainLoopEngine;
pub(crate) use window_handle::WindowHandle;

use crate::{AuthCredentials, Error, Result};

// Re-export TOFU types for public API
pub use tls::{TofuPrompt, TofuVerifier, VerifyMode};

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub tls_host: Option<String>,
    /// TLS verification mode.
    pub verify_mode: Option<VerifyMode>,
    pub credentials: AuthCredentials,
    /// Enable automatic reconnection on connection loss. Default: false.
    /// When enabled, the client will emit `Reconnecting`, `Reconnected`, and
    /// `ReconnectionFailed` events that the application must handle.
    pub auto_reconnection: bool,
}

impl ConnectConfig {
    /// Set fingerprint-based TLS verification.
    pub fn set_tls_fingerprint(&mut self, fingerprint: &str) -> Result<()> {
        // Validate fingerprint format
        if fingerprint::parse(fingerprint).is_none() {
            return Err(Error::TlsInvalidFingerprint);
        }
        self.verify_mode = Some(VerifyMode::Fingerprint {
            fingerprint: fingerprint.to_string(),
        });
        Ok(())
    }

    /// Enable TOFU (Trust-On-First-Use) verification with a prompt callback.
    pub fn enable_tofu<F>(&mut self, prompt_callback: F) -> Result<()>
    where
        F: Fn(TofuPrompt) -> bool + Send + Sync + 'static,
    {
        let verifier = TofuVerifier::new(prompt_callback)?;
        self.verify_mode = Some(VerifyMode::Tofu {
            verifier: Arc::new(verifier),
        });
        Ok(())
    }

    /// Enable TOFU with auto-accept for new hosts.
    pub fn enable_tofu_auto_accept(&mut self) -> Result<()> {
        let verifier = TofuVerifier::auto_accept()?;
        self.verify_mode = Some(VerifyMode::Tofu {
            verifier: Arc::new(verifier),
        });
        Ok(())
    }

    /// Enable strict known_hosts mode (reject unknown hosts).
    pub fn enable_known_hosts_strict(&mut self) -> Result<()> {
        let verifier = TofuVerifier::strict()?;
        self.verify_mode = Some(VerifyMode::Tofu {
            verifier: Arc::new(verifier),
        });
        Ok(())
    }

    /// Skip TLS verification (DANGER - testing only).
    pub fn skip_tls_verification(&mut self) {
        self.verify_mode = Some(VerifyMode::SkipVerification);
    }
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
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) tls_host: Option<String>,
    pub(crate) verify_mode: Option<VerifyMode>,
    pub(crate) credentials: AuthCredentials,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        Ok(Self {
            host: connect_config.host,
            port: connect_config.port,
            tls_host: connect_config.tls_host.clone(),
            verify_mode: connect_config.verify_mode,
            credentials: connect_config.credentials,
        })
    }
}
