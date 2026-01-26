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

//! HTTP client configuration with custom TLS verification.

use log::warn;
use reqwest::RequestBuilder;

use super::tls::{self, VerifyMode};
use crate::{ConnectionParams, Error, Result};

pub(crate) fn create_host(conn_params: &ConnectionParams) -> String {
    let host = conn_params.tls_host.as_ref().unwrap_or(&conn_params.host);
    format!("{host}:{port}", port = conn_params.port)
}

pub(crate) fn create_reqwest(conn_params: ConnectionParams) -> Result<reqwest::Client> {
    let client_builder = reqwest::ClientBuilder::new().cookie_store(true);

    // Apply hostname resolution override if tls_host is set
    let client_builder = if let Some(tls_host) = &conn_params.tls_host {
        client_builder.resolve(tls_host, conn_params.addr)
    } else {
        client_builder
    };

    // Apply TLS configuration based on verify_mode
    let client = if let Some(verify_mode) = &conn_params.verify_mode {
        match verify_mode {
            VerifyMode::RootCert { root_certificate } => {
                // Use reqwest's built-in root certificate handling
                let certificate = reqwest::Certificate::from_der(root_certificate)?;
                client_builder.add_root_certificate(certificate).build()?
            }
            VerifyMode::Fingerprint { .. } | VerifyMode::Tofu { .. } => {
                // Use custom rustls config for fingerprint/TOFU verification
                let rustls_config = tls::build_client_config(verify_mode).map_err(|e| {
                    warn!("Failed to build TLS config: {e}");
                    Error::TlsVerificationFailed
                })?;

                client_builder
                    .use_preconfigured_tls(rustls_config)
                    .build()?
            }
            VerifyMode::SkipVerification => {
                // Use reqwest's danger_accept_invalid_certs
                warn!("HTTP client: TLS verification DISABLED");
                client_builder.danger_accept_invalid_certs(true).build()?
            }
        }
    } else {
        // No TLS configuration - use system defaults
        client_builder.build()?
    };

    Ok(client)
}

pub(crate) fn configure_request(builder: RequestBuilder) -> RequestBuilder {
    builder
}
