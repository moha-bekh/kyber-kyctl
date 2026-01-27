// Project Kyber: tls.rs
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

use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

use log::error;

use crate::{Error, Result, TlsRootCert};

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub(crate) root_certificate: Vec<u8>,
}

fn get_resource_path(resource_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(resource_name);

        path
    } else {
        PathBuf::from(resource_name)
    }
}

pub(crate) fn load_tls_config(root_cert: &TlsRootCert) -> Result<Config> {
    match root_cert {
        TlsRootCert::Path(root_cert_path) => {
            let root_cert_path = get_resource_path(root_cert_path);

            let root_cert_file = File::open(&root_cert_path).map_err(|e| {
                error!("Failed to open file {root_cert_path:?}: {e:?}");
                Error::FileNotFound(root_cert_path.clone())
            })?;

            let mut root_cert_reader = BufReader::new(&root_cert_file);

            let root_certificate = match rustls_pemfile::read_one(&mut root_cert_reader)? {
                Some(rustls_pemfile::Item::X509Certificate(cert)) => cert,
                _ => {
                    error!("{root_cert_path:?} doesn't contain a X509 Certificate");
                    return Err(Error::TlsInvalidFile);
                }
            };

            Ok(Config {
                root_certificate: root_certificate.to_vec(),
            })
        }
        TlsRootCert::Bytes(bytes) => {
            let root_certificate = match rustls_pemfile::read_one_from_slice(bytes) {
                Ok(Some((rustls_pemfile::Item::X509Certificate(cert), _))) => cert,
                _ => {
                    error!("given bytes don't contain a X509 Certificate");
                    return Err(Error::TlsInvalidFile);
                }
            };

            Ok(Config {
                root_certificate: root_certificate.to_vec(),
            })
        }
    }
}
