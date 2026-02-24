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

use actix_web::cookie;

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    same_site: bool,
}

impl Config {
    pub fn same_site(&self) -> cookie::SameSite {
        if self.same_site {
            cookie::SameSite::Strict
        } else {
            cookie::SameSite::None
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self { same_site: true }
    }
}

pub fn build_cors() -> actix_cors::Cors {
    actix_cors::Cors::default()
        .allowed_methods(vec!["GET", "POST"])
        .supports_credentials()
        .allowed_header(actix_web::http::header::CONTENT_TYPE)
        .allowed_header(actix_web::http::header::AUTHORIZATION)
        .allow_any_origin()
}
