// Project Kyber: webtransport.rs
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

use rustls::pki_types;
use sha2::{Digest, Sha256};
use time::Duration;

use crate::{Error, Result};

pub(super) struct Cert {
    pub(super) cert_der: pki_types::CertificateDer<'static>,
    pub(super) cert_digest: String,
    pub(super) key_der: pki_types::PrivateKeyDer<'static>,
}

pub(super) fn gen_webtransport_cert() -> Result<Cert> {
    const COMMON_NAME: &str = "localhost";
    let now = time::OffsetDateTime::now_utc();

    // Generate certificate.
    // Requirements: https://developer.mozilla.org/en-US/docs/Web/API/WebTransport/WebTransport#servercertificatehashes
    let mut dname = rcgen::DistinguishedName::new();
    dname.push(rcgen::DnType::CommonName, COMMON_NAME);

    let mut cert_params = rcgen::CertificateParams::new(vec![COMMON_NAME.to_string()])?;
    cert_params.distinguished_name = dname;
    cert_params.not_before = now
        .checked_sub(Duration::days(2))
        .ok_or(Error::DateOperationOverflow)?;
    cert_params.not_after = now
        .checked_add(Duration::days(2))
        .ok_or(Error::DateOperationOverflow)?;

    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let cert = cert_params.self_signed(&key_pair)?;

    // Extract keys
    let cert_der = cert.der().clone();
    let key_der = pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()).into();

    // Create key digest
    let mut hasher = Sha256::new();
    hasher.update(&cert_der);
    let cert_digest = format!("{:x}", hasher.finalize());

    Ok(Cert {
        cert_der,
        cert_digest,
        key_der,
    })
}
