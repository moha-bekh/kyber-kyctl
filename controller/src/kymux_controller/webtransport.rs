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
