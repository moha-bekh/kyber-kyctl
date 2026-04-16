// Project Kyber: expiration.rs
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

//! Background task for enforcing session expiration.
//!
//! This module implements a periodic cleanup task that terminates expired sessions
//! every 30 seconds. It complements the request-time expiration checks in the
//! Identity middleware.
//!
//! ## Why both background task and middleware?
//!
//! - **Middleware**: Validates expiration on HTTP requests
//! - **Background task**: Handles WebSocket-only connections that don't make HTTP requests
//!
//! Without the background task, a user with only a WebSocket connection could continue
//! streaming indefinitely even after their session expires, since no HTTP requests would
//! trigger the middleware validation.
//!
//! ## Cleanup Process
//!
//! Every 30 seconds, the task:
//! 1. Iterates through all client sessions
//! 2. Checks if each session has an expiration time (skipped for Basic auth)
//! 3. Compares expiration to current UTC time
//! 4. If expired: Calls `Controller::stop()` to terminate that session's streaming services
//! 5. Cleans up expired pending authentication tokens

use std::time::Duration;

use actix_web::web;
use log::warn;
use time::OffsetDateTime;

use crate::{utils::Task, Controller};

/// Creates and starts a new expiration monitoring task.
///
/// The task spawns immediately and begins checking for expired sessions
/// every 30 seconds.
///
/// # Parameters
/// - `ctrl`: Shared reference to the Controller
///
/// # Returns
/// An `ExpirationTask` handle that can be used to stop the task
pub(crate) fn expiration_task(ctrl: web::Data<Controller>) -> Task {
    Task::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            check_expiration(&ctrl).await;
        }
    })
}

/// Checks if any sessions have expired and terminates them.
///
/// ## Process:
///
/// 1. Lock the controller (brief - no I/O while locked)
/// 2. Iterate through all client sessions
/// 3. Check if session has expiration time (skip for Basic auth)
/// 4. Compare expiration to current UTC time
/// 5. If expired: Call `Controller::stop()` to terminate that session's streaming services
/// 6. Clean up any expired pending authentication tokens
async fn check_expiration(ctrl: &web::Data<Controller>) {
    // Collect expired sessions and extract cleanup data
    let now = OffsetDateTime::now_utc();
    ctrl.remove_sessions_if(|s| {
        let expired = s.expiration.is_some_and(|exp| exp < now);
        if expired {
            warn!("Killing expired session \"{}\"", s.uid);
        }
        expired
    })
    .await;

    // Cleanup expired pending auth tokens in shared Kymux infrastructure
    ctrl.shared_kymux_infra.lock().await.cleanup_expired().await;
}
