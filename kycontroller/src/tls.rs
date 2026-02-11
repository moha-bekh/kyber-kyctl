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

use log::{error, warn};

use crate::utils::get_resource_path;
use crate::{Error, Result};

use rustls::pki_types;

pub(crate) struct Config {
    pub(crate) cert_chain: Vec<pki_types::CertificateDer<'static>>,
    pub(crate) private_key: pki_types::PrivateKeyDer<'static>,
}

impl Clone for Config {
    fn clone(&self) -> Self {
        Self {
            cert_chain: self.cert_chain.clone(),
            private_key: self.private_key.clone_key(),
        }
    }
}

fn open_reader(path: &str) -> Result<BufReader<File>> {
    let path = get_resource_path(path);

    let f = File::open(&path).map_err(|e| {
        error!("Failed to open file {path:?}: {e:?}");
        Error::FileNotFound { path }
    })?;

    Ok(BufReader::new(f))
}

pub(crate) fn load_config(cert_path: &str, key_path: &str) -> Result<Config> {
    use rustls_pemfile::Item;

    // Load certificate
    let mut cert_reader = open_reader(cert_path)?;

    let cert_chain: Vec<_> = rustls_pemfile::read_all(&mut cert_reader)
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|item| match item {
            Item::X509Certificate(cert) => Some(cert),
            _ => {
                warn!("{cert_path}: Ignore non X509 Certificate item");
                None
            }
        })
        .collect();

    // Load key
    let mut key_reader = open_reader(key_path)?;

    let private_key = match rustls_pemfile::read_one(&mut key_reader)? {
        Some(rustls_pemfile::Item::Pkcs1Key(key)) => pki_types::PrivateKeyDer::Pkcs1(key),
        Some(rustls_pemfile::Item::Pkcs8Key(key)) => pki_types::PrivateKeyDer::Pkcs8(key),
        _ => {
            error!("{key_path} doesn't contain a PKCS1/PKCS8 Key");
            return Err(Error::TlsInvalidFile);
        }
    };

    // Build configuration
    Ok(Config {
        cert_chain,
        private_key,
    })
}
