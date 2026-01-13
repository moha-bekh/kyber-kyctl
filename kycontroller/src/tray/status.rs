// Project Kyber: status.rs
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

//! Tray status and command types.
//!
//! These types are only used on Windows with the win32-tray feature enabled.

#![allow(dead_code)]

/// Status of the Kyber controller for display in the system tray.
#[derive(Debug, Clone)]
pub struct TrayStatus {
    /// List of connected usernames.
    pub clients: Vec<String>,
}

impl TrayStatus {
    /// Create an idle status with no clients.
    pub fn idle() -> Self {
        Self { clients: vec![] }
    }

    /// Returns a human-readable tooltip string for this status.
    pub fn tooltip(&self) -> String {
        match self.clients.len() {
            0 => "Kyber - Ready".to_string(),
            1 => format!("Kyber - {} connected", self.clients[0]),
            n => format!("Kyber - {} clients connected", n),
        }
    }
}

/// Commands that can be sent from the tray to the main application.
#[derive(Debug, Clone)]
pub enum TrayCommand {
    /// Request to stop the controller gracefully.
    Stop,
    /// Request to restart the controller.
    Restart,
}
