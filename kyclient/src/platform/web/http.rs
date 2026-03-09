// Project Kyber: http.rs
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

use reqwest::RequestBuilder;

use crate::{ConnectionParams, Result};

pub(crate) fn create_host(conn_params: &ConnectionParams) -> String {
    crate::http::format_host_for_url(&conn_params.host, conn_params.port)
}

pub(crate) fn create_reqwest(_conn_params: ConnectionParams) -> Result<reqwest::Client> {
    let client = reqwest::ClientBuilder::new().build()?;

    Ok(client)
}

pub(crate) fn configure_request(builder: RequestBuilder) -> RequestBuilder {
    builder.fetch_credentials_include()
}
