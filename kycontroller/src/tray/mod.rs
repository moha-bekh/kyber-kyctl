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

//! System tray support.
//!
//! This module provides a system tray icon with status updates
//! and user commands (Stop/Restart).
//!
//! # Platform Support
//!
//! - Windows: Full support with tray icon, menu, and status updates
//! - Linux/macOS: Runs server without tray (logs warning)
//!
//! # Usage
//!
//! ```ignore
//! // In main.rs, use the high-level run_with_tray function:
//! tray::run_with_tray(server, ctrl.clone()).await?;
//! ```

mod platform;
mod status;

// These are only used on Windows with win32-tray feature
#[allow(unused_imports)]
pub use platform::{run_with_tray, TrayHandle};
pub use status::TrayStatus;
