// Project Kyber: stub.rs
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

//! Stub tray implementation for unsupported platforms.
//!
//! This module provides no-op implementations for platforms without tray support.
//! The types are defined to maintain API compatibility but are not actually used.

#![allow(dead_code)]
#![allow(unused_imports)]

use crate::error::Result;

use super::super::status::{TrayCommand, TrayStatus};

use log::warn;
use tokio::sync::mpsc;

/// Stub handle for unsupported platforms.
pub struct TrayHandle {
    _shutdown_tx: mpsc::Sender<TrayCommand>,
}

impl TrayHandle {
    /// Update the tray icon status (no-op on unsupported platforms).
    pub async fn set_status(&self, _status: TrayStatus) {
        // No-op
    }

    /// Signal the Windows service to stop (no-op on unsupported platforms).
    pub fn signal_service_stop(&self) {
        // No-op
    }

    /// Shutdown the tray (no-op on unsupported platforms).
    pub async fn shutdown(&mut self) {
        // No-op
    }
}

/// Spawn the system tray (stub for unsupported platforms).
///
/// Returns a dummy handle and a channel that will never receive signals.
pub fn spawn() -> Result<(TrayHandle, mpsc::Receiver<TrayCommand>)> {
    warn!("System tray not supported on this platform");
    let (_shutdown_tx, shutdown_rx) = mpsc::channel(1);
    Ok((TrayHandle { _shutdown_tx }, shutdown_rx))
}

/// Wait for a service shutdown signal (stub for unsupported platforms).
///
/// Returns a future that never resolves since service events are Windows-only.
pub async fn wait_service_shutdown() {
    std::future::pending::<()>().await;
}
