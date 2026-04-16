// Project Kyber: mod.rs
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

//! System tray support.
//!
//! This module provides a system tray icon with status updates
//! and user commands (Stop/Restart).
//!
//! # Architecture
//!
//! The tray runs in a dedicated OS thread (not a tokio task) because:
//! - Windows requires a message pump (GetMessage/DispatchMessage loop) for tray icons
//! - The message pump is inherently blocking and must run on the creating thread
//! - Menu libraries require the event loop to run on the same thread that created them
//!
//! Communication with async code uses:
//! - Platform-specific events for efficient shutdown signaling
//! - tokio::sync::mpsc channels for status updates and tray commands
//!
//! # Platform Support
//!
//! - Windows: Full support with tray icon, menu, and status updates
//! - Linux/macOS: Stub implementation (logs warning)
//!
//! # Usage
//!
//! ```ignore
//! // In main.rs:
//! let server_handle = server.handle();
//!
//! tokio::select! {
//!     result = server => {
//!         if let Err(e) = result { warn!("Server error: {e}"); }
//!     }
//!     _ = tray::run(ctrl.clone(), server_handle.clone()) => {}
//! }
//!
//! ctrl.lock().await.shutdown_tray().await;
//! ```

mod platform;
mod status;

// These are only used on Windows with win32-tray feature
#[allow(unused_imports)]
pub use platform::{spawn, wait_service_shutdown, TrayHandle};
#[allow(unused_imports)]
pub use status::{TrayCommand, TrayStatus};

/// Whether the system tray is supported on this platform.
pub fn is_supported() -> bool {
    cfg!(all(windows, feature = "win32-tray"))
}

use crate::controller::Controller;

use actix_web::dev::ServerHandle;
use log::{error, info};
use tokio::sync::Mutex;

/// Spawn the system tray and wait for a command.
///
/// Spawns the tray icon, sets initial status, stores the handle in the
/// controller, then blocks until a command is received from the tray
/// menu or the Windows service.
///
/// On signal: calls `signal_service_stop()` (for Stop) and stops the server.
/// Tray initialization failure is non-fatal — the controller continues
/// without a tray icon. The tray will re-create itself when Explorer
/// broadcasts the TaskbarCreated message (e.g. after screen unlock).
pub async fn run(ctrl: actix_web::web::Data<Mutex<Controller>>, server_handle: ServerHandle) {
    let mut shutdown_rx = match spawn() {
        Ok((tray_handle, rx)) => {
            info!("System tray initialized");
            tray_handle.set_status(TrayStatus::idle());
            ctrl.lock().await.set_tray_handle(tray_handle);
            rx
        }
        Err(e) => {
            error!("Failed to initialize system tray: {e}");
            return;
        }
    };

    let Some(signal) = shutdown_rx.recv().await else {
        error!("Tray thread exited unexpectedly");
        return;
    };

    match signal {
        TrayCommand::Stop => {
            info!("Stop signal received");
            ctrl.lock().await.signal_service_stop();
            server_handle.stop(false).await;
        }
        TrayCommand::Restart => {
            info!("Restart signal received");
            server_handle.stop(false).await;
        }
    }
}
