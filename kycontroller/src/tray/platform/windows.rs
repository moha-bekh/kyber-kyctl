// Project Kyber: windows.rs
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

//! Windows-specific system tray implementation.
//!
//! This module contains all Windows-specific code for the system tray,
//! including the message pump, tray icon creation, and Windows service integration.
//!
//! # Native Shell_NotifyIconW Implementation
//!
//! This module uses direct Windows Shell_NotifyIconW API calls with NIF_GUID
//! to enable cross-process icon identification. This allows:
//! - New controller instances to clean up orphan icons from crashed instances
//! - Reliable icon removal regardless of how the process exits
//!
//! The tray-icon crate is NOT used for icon management because it creates icons
//! without NIF_GUID, making orphan cleanup impossible.

use super::super::status::{TrayCommand, TrayStatus};
use crate::error::{Error, Result};

use log::{info, warn};
use std::thread::JoinHandle;
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::SetEvent;

/// Wrapper for Windows HANDLE that implements Send + Sync and auto-closes on drop.
///
/// Windows event handles are safe to use across threads (they are kernel objects
/// with reference counting), but raw pointers don't implement Send/Sync.
struct SendHandle(windows_sys::Win32::Foundation::HANDLE);

unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

impl SendHandle {
    fn as_raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }

    fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl Drop for SendHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Safety: handle is valid and we own it
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Handle to control the system tray from async code.
///
/// All methods are async-safe despite the tray running on a dedicated thread.
/// Communication uses Windows events for shutdown and tokio channels for status.
pub struct TrayHandle {
    /// Send status updates to the tray thread.
    status_tx: mpsc::Sender<TrayStatus>,
    /// Windows event for shutdown signaling.
    shutdown_event: SendHandle,
    /// Windows event for waking to check status updates.
    wake_event: SendHandle,
    /// Join handle for the tray thread.
    thread_handle: Option<JoinHandle<()>>,
}

impl TrayHandle {
    /// Create a new TrayHandle.
    fn new(
        status_tx: mpsc::Sender<TrayStatus>,
        shutdown_event: SendHandle,
        wake_event: SendHandle,
        thread_handle: JoinHandle<()>,
    ) -> Self {
        Self {
            status_tx,
            shutdown_event,
            wake_event,
            thread_handle: Some(thread_handle),
        }
    }

    /// Update the tray icon status (tooltip and potentially icon).
    ///
    /// This is async and will not block the caller.
    pub async fn set_status(&self, status: TrayStatus) {
        if let Err(e) = self.status_tx.send(status).await {
            warn!("Failed to send status to tray: {e}");
            return;
        }

        // Signal the wake event to notify the tray thread
        // Safety: wake_event is a valid handle created by CreateEventW
        unsafe { SetEvent(self.wake_event.as_raw()) };
    }

    /// Shutdown the tray thread and wait for it to finish.
    ///
    /// This ensures the tray icon is properly removed before process exit.
    pub async fn shutdown(&mut self) {
        info!("Shutting down tray...");

        // Signal the tray thread to exit
        // Safety: shutdown_event is a valid handle created by CreateEventW
        unsafe { SetEvent(self.shutdown_event.as_raw()) };

        // Wait for the thread to finish
        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(()) => info!("Tray thread exited cleanly"),
                Err(_) => warn!("Tray thread panicked during shutdown"),
            }
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        // Signal the tray thread to exit if not already done
        if !self.shutdown_event.is_null() {
            // Safety: shutdown_event is a valid handle created by CreateEventW
            unsafe { SetEvent(self.shutdown_event.as_raw()) };
        }

        // Wait for the thread to finish (blocking, but Drop is synchronous)
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        // Event handles are closed automatically by SendHandle::drop()
    }
}

/// Embedded Kyber icon (PNG format).
const ICON_BYTES: &[u8] = include_bytes!("../../../res/kyber_tray.png");

/// Menu item IDs
const MENU_STOP_ID: &str = "stop";
const MENU_RESTART_ID: &str = "restart";

/// Custom message for tray icon callbacks.
const WM_TRAYICON: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_USER + 1;

/// Window class name for the hidden tray window.
const TRAY_WINDOW_CLASS: &str = "KyberTrayWindow";

/// Fixed GUID for the tray icon.
/// Using NIF_GUID allows Windows to identify the icon across process restarts,
/// enabling cleanup of orphan icons when a new controller starts.
/// GUID: {7B3F8E2A-1C4D-4E5F-9A6B-8C7D0E1F2A3B}
const TRAY_ICON_GUID: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x7B3F8E2A,
    data2: 0x1C4D,
    data3: 0x4E5F,
    data4: [0x9A, 0x6B, 0x8C, 0x7D, 0x0E, 0x1F, 0x2A, 0x3B],
};

/// Wait result from the tray event loop.
enum WaitResult {
    /// Shutdown event was signaled.
    Shutdown,
    /// Wake event was signaled (status update available).
    Wake,
    /// Windows messages are available.
    WindowsMessage,
}

/// Spawn the system tray thread.
///
/// Returns a handle to control the tray and sends commands to the provided channel.
pub fn spawn(_command_tx: mpsc::Sender<TrayCommand>) -> Result<TrayHandle> {
    // TODO: Implement tray spawning
    Err(Error::TrayNotSupported)
}
