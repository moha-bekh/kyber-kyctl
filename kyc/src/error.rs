// Project Kyber: error.rs
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

/// Errors that can occur during OIDC flows.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// OIDC provider is unreachable or returned an invalid response.
    #[error("OIDC discovery failed: {0}")]
    Discovery(String),

    /// Failed to bind the loopback listener for the redirect callback.
    #[error("loopback listener failed: {0}")]
    Listener(#[from] std::io::Error),

    /// The browser callback timed out or contained invalid data.
    #[error("callback error: {0}")]
    Callback(String),

    /// CSRF state or nonce validation failed.
    #[error("security validation failed: {0}")]
    Validation(String),

    /// Token exchange or refresh request failed.
    #[error("token exchange failed: {0}")]
    TokenExchange(String),

    /// JWT parsing error.
    #[error("JWT error: {0}")]
    Jwt(String),
}
