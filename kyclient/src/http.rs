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

use reqwest::IntoUrl;

use crate::platform::http as platform_http;
use crate::{ConnectionParams, Result};

/// Formats a host and port for use in URLs, wrapping IPv6 addresses in brackets.
pub(crate) fn format_host_for_url(host: &str, port: u16) -> String {
    use std::net::IpAddr;
    if let Ok(IpAddr::V6(_)) = host.parse::<IpAddr>() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub(crate) struct HttpClient {
    base_url: String,
    client: reqwest::Client,
    host: String,
}

impl HttpClient {
    pub(crate) fn new(conn_params: ConnectionParams) -> Result<Self> {
        let host = platform_http::create_host(&conn_params);
        let base_url = format!("https://{host}");

        let client = platform_http::create_reqwest(conn_params)?;

        Ok(Self {
            base_url,
            client,
            host,
        })
    }

    pub(crate) fn get<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        platform_http::configure_request(self.client.get(url))
    }

    pub(crate) fn post<U: IntoUrl>(&self, url: U) -> reqwest::RequestBuilder {
        platform_http::configure_request(self.client.post(url))
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    #[allow(dead_code)]
    pub(crate) fn host(&self) -> &str {
        &self.host
    }
}
