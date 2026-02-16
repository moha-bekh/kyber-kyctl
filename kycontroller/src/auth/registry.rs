// Project Kyber: registry.rs
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

use crate::auth::{self, Backend, Config};

/// Container for enabled authentication backends.
///
/// Backends can be disabled at runtime via configuration even when compiled in.
pub struct BackendRegistry {
    #[cfg(feature = "auth-basic")]
    basic: Option<auth::basic::Basic>,
    #[cfg(feature = "auth-jwt")]
    jwt: Option<auth::jwt::Jwt>,
}

impl BackendRegistry {
    pub fn new(config: &Config) -> Self {
        Self {
            #[cfg(feature = "auth-basic")]
            basic: config
                .basic
                .enabled
                .then_some(auth::basic::Basic::new(&config.basic.inner)),
            #[cfg(feature = "auth-jwt")]
            jwt: config
                .jwt
                .enabled
                .then_some(auth::jwt::Jwt::new(&config.jwt.inner)),
        }
    }

    /// Returns the backend for the given method, or `None` if disabled.
    pub fn backend(&self, method: auth::Method) -> Option<&dyn Backend> {
        match method {
            #[cfg(feature = "auth-basic")]
            auth::Method::Basic => self.basic.as_ref().map(|b| b as &dyn Backend),
            #[cfg(feature = "auth-jwt")]
            auth::Method::Jwt => self.jwt.as_ref().map(|b| b as &dyn Backend),
        }
    }
}
