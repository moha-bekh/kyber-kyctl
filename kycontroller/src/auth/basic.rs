// Project Kyber: basic.rs
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

use base64::{prelude::BASE64_STANDARD, Engine};

use super::{Backend, Error, Session};

pub struct Basic;

impl Backend for Basic {
    /// Authenticates a user without password validation.
    ///
    /// # Parameters
    /// - `username`: The username to authenticate (no validation performed)
    ///
    /// # Returns
    /// A session with no expiration time (valid indefinitely until logout)
    fn authenticate(&self, credentials: &str) -> Result<Session, Error> {
        let decoded = BASE64_STANDARD
            .decode(credentials)
            .or(Err(Error::InvalidCredentialsFormat))?;

        let username = std::str::from_utf8(&decoded)
            .or(Err(Error::InvalidCredentialsFormat))?
            .split(':')
            .next()
            .ok_or(Error::MissingField {
                field: "username".to_owned(),
            })?
            .to_owned();

        Ok(Session {
            username,
            expiration: None,
        })
    }
}
