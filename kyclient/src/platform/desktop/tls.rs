// Project Kyber: tls.rs
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

//! TLS configuration and verification for control plane connections.

use std::path::PathBuf;
use std::sync::Arc;

use log::{error, info, warn};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use super::fingerprint;
use super::known_hosts::KnownHosts;
use crate::Result;

/// Resolve a resource path relative to the executable.
///
/// If the path is relative, it's resolved relative to the directory containing
/// the executable. This allows clients to bundle resources next to the binary.
pub(crate) fn get_resource_path(resource_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(resource_name);
        path
    } else {
        PathBuf::from(resource_name)
    }
}

/// Common signature schemes supported by all custom verifiers.
fn supported_verify_schemes() -> Vec<SignatureScheme> {
    vec![
        SignatureScheme::RSA_PKCS1_SHA256,
        SignatureScheme::RSA_PKCS1_SHA384,
        SignatureScheme::RSA_PKCS1_SHA512,
        SignatureScheme::ECDSA_NISTP256_SHA256,
        SignatureScheme::ECDSA_NISTP384_SHA384,
        SignatureScheme::ECDSA_NISTP521_SHA512,
        SignatureScheme::RSA_PSS_SHA256,
        SignatureScheme::RSA_PSS_SHA384,
        SignatureScheme::RSA_PSS_SHA512,
        SignatureScheme::ED25519,
    ]
}

/// TLS verification mode for control plane connections.
#[derive(Clone)]
pub enum VerifyMode {
    /// Verify against a custom root certificate.
    RootCert { root_certificate: Vec<u8> },
    /// Verify against a specific certificate fingerprint.
    Fingerprint { fingerprint: String },
    /// Trust-On-First-Use: accept new hosts, reject changed certificates.
    Tofu { verifier: Arc<TofuVerifier> },
    /// Skip verification (DANGER - testing only).
    SkipVerification,
}

impl std::fmt::Debug for VerifyMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyMode::RootCert { .. } => write!(f, "RootCert"),
            VerifyMode::Fingerprint { fingerprint } => {
                write!(f, "Fingerprint({})", fingerprint)
            }
            VerifyMode::Tofu { .. } => write!(f, "Tofu"),
            VerifyMode::SkipVerification => write!(f, "SkipVerification"),
        }
    }
}

/// TOFU prompt types for user interaction.
#[derive(Clone, Debug)]
pub enum TofuPrompt {
    /// New unknown host encountered.
    UnknownHost {
        hostname: String,
        fingerprint: String,
    },
    /// Host's certificate has changed from stored fingerprint.
    CertificateChanged {
        hostname: String,
        expected_fingerprint: String,
        actual_fingerprint: String,
    },
}

/// Error types for TOFU verification.
#[derive(Debug)]
pub enum TofuError {
    /// Certificate fingerprint doesn't match stored value.
    FingerprintMismatch { expected: String, actual: String },
    /// Host not in known_hosts and user rejected.
    UnknownHost {
        hostname: String,
        fingerprint: String,
    },
}

/// TOFU verifier that manages known hosts and user prompts.
pub struct TofuVerifier {
    known_hosts: KnownHosts,
    prompt_callback: Box<dyn Fn(TofuPrompt) -> bool + Send + Sync>,
}

impl TofuVerifier {
    /// Create a new TOFU verifier with a prompt callback.
    ///
    /// The callback blocks the connection thread during TLS handshake.
    pub fn new<F>(prompt_callback: F) -> Result<Self>
    where
        F: Fn(TofuPrompt) -> bool + Send + Sync + 'static,
    {
        let known_hosts = KnownHosts::load()?;
        Ok(Self {
            known_hosts,
            prompt_callback: Box::new(prompt_callback),
        })
    }

    /// Create a TOFU verifier that auto-accepts new hosts (no user interaction).
    pub fn auto_accept() -> Result<Self> {
        Self::new(|prompt| match prompt {
            TofuPrompt::UnknownHost { hostname, .. } => {
                info!("TOFU: Auto-accepting new host '{}'", hostname);
                true
            }
            TofuPrompt::CertificateChanged { hostname, .. } => {
                warn!("TOFU: Rejecting changed certificate for '{}'", hostname);
                false
            }
        })
    }

    /// Create a TOFU verifier for strict mode (only accepts known hosts).
    pub fn strict() -> Result<Self> {
        Self::new(|prompt| match prompt {
            TofuPrompt::UnknownHost { hostname, .. } => {
                warn!("TOFU strict: Rejecting unknown host '{}'", hostname);
                false
            }
            TofuPrompt::CertificateChanged { hostname, .. } => {
                warn!(
                    "TOFU strict: Rejecting changed certificate for '{}'",
                    hostname
                );
                false
            }
        })
    }

    /// Verify a certificate against known hosts.
    pub fn verify(&self, hostname: &str, cert_der: &[u8]) -> std::result::Result<(), TofuError> {
        let actual_fingerprint = fingerprint::calculate(cert_der);

        if let Some(stored_fingerprint) = self.known_hosts.get(hostname) {
            // Host is known - verify fingerprint matches
            if stored_fingerprint.eq_ignore_ascii_case(&actual_fingerprint) {
                info!(
                    "TOFU: Certificate matches known fingerprint for '{}'",
                    hostname
                );
                Ok(())
            } else {
                // Certificate changed!
                let accepted = (self.prompt_callback)(TofuPrompt::CertificateChanged {
                    hostname: hostname.to_string(),
                    expected_fingerprint: stored_fingerprint.clone(),
                    actual_fingerprint: actual_fingerprint.clone(),
                });

                if accepted {
                    // User accepted the new certificate - update known_hosts
                    if let Err(e) = self.known_hosts.insert(hostname, &actual_fingerprint) {
                        warn!("Failed to update known_hosts: {:?}", e);
                    }
                    if let Err(e) = self.known_hosts.save() {
                        warn!("Failed to save known_hosts: {:?}", e);
                    }
                    Ok(())
                } else {
                    Err(TofuError::FingerprintMismatch {
                        expected: stored_fingerprint,
                        actual: actual_fingerprint,
                    })
                }
            }
        } else {
            // New host - prompt user
            let accepted = (self.prompt_callback)(TofuPrompt::UnknownHost {
                hostname: hostname.to_string(),
                fingerprint: actual_fingerprint.clone(),
            });

            if accepted {
                // Save the fingerprint
                if let Err(e) = self.known_hosts.insert(hostname, &actual_fingerprint) {
                    warn!("Failed to save to known_hosts: {:?}", e);
                }
                if let Err(e) = self.known_hosts.save() {
                    warn!("Failed to save known_hosts: {:?}", e);
                }
                info!("TOFU: Saved fingerprint for new host '{}'", hostname);
                Ok(())
            } else {
                Err(TofuError::UnknownHost {
                    hostname: hostname.to_string(),
                    fingerprint: actual_fingerprint,
                })
            }
        }
    }

    /// Get the underlying known hosts storage.
    pub fn known_hosts(&self) -> &KnownHosts {
        &self.known_hosts
    }
}

impl std::fmt::Debug for TofuVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TofuVerifier")
            .field("known_hosts", &self.known_hosts)
            .finish()
    }
}

/// Custom rustls ServerCertVerifier for fingerprint verification.
#[derive(Debug)]
pub struct FingerprintVerifier {
    expected_fingerprint: String,
}

impl FingerprintVerifier {
    pub fn new(fingerprint: &str) -> Self {
        Self {
            expected_fingerprint: fingerprint.to_string(),
        }
    }
}

impl ServerCertVerifier for FingerprintVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let actual = fingerprint::calculate(end_entity.as_ref());

        if actual.eq_ignore_ascii_case(&self.expected_fingerprint) {
            info!("Fingerprint verification successful");
            Ok(ServerCertVerified::assertion())
        } else {
            error!(
                "Fingerprint mismatch: expected {}, got {}",
                self.expected_fingerprint, actual
            );
            Err(rustls::Error::General(
                "Certificate fingerprint mismatch".to_string(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .ok_or_else(|| rustls::Error::General("no crypto provider".to_string()))?
                .signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .ok_or_else(|| rustls::Error::General("no crypto provider".to_string()))?
                .signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms.supported_schemes())
            .unwrap_or_default()
    }
}

/// Custom rustls ServerCertVerifier for TOFU verification.
pub struct TofuServerCertVerifier {
    verifier: Arc<TofuVerifier>,
}

impl TofuServerCertVerifier {
    pub fn new(verifier: Arc<TofuVerifier>) -> Self {
        Self { verifier }
    }
}

impl std::fmt::Debug for TofuServerCertVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TofuServerCertVerifier").finish()
    }
}

impl ServerCertVerifier for TofuServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let hostname = match server_name {
            ServerName::DnsName(name) => name.as_ref().to_string(),
            ServerName::IpAddress(addr) => std::net::IpAddr::from(*addr).to_string(),
            _ => {
                return Err(rustls::Error::General(
                    "TOFU requires DNS or IP server name".to_string(),
                ));
            }
        };

        match self.verifier.verify(&hostname, end_entity.as_ref()) {
            Ok(()) => Ok(ServerCertVerified::assertion()),
            Err(TofuError::FingerprintMismatch { expected, actual }) => {
                Err(rustls::Error::General(format!(
                    "Certificate changed: expected {}, got {}",
                    expected, actual
                )))
            }
            Err(TofuError::UnknownHost { hostname, .. }) => Err(rustls::Error::General(format!(
                "Unknown host '{}' rejected",
                hostname
            ))),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .ok_or_else(|| rustls::Error::General("no crypto provider".to_string()))?
                .signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &CryptoProvider::get_default()
                .ok_or_else(|| rustls::Error::General("no crypto provider".to_string()))?
                .signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        CryptoProvider::get_default()
            .map(|p| p.signature_verification_algorithms.supported_schemes())
            .unwrap_or_default()
    }
}

/// Skip verification verifier (DANGER - testing only).
#[derive(Debug)]
pub struct SkipVerifier;

impl ServerCertVerifier for SkipVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        warn!("TLS verification SKIPPED - connection is NOT secure!");
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_verify_schemes()
    }
}

/// Build a rustls ClientConfig for the given verification mode.
pub fn build_client_config(
    mode: &VerifyMode,
) -> std::result::Result<rustls::ClientConfig, rustls::Error> {
    let config = match mode {
        VerifyMode::RootCert { root_certificate } => {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(CertificateDer::from(root_certificate.clone()))?;

            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        }
        VerifyMode::Fingerprint { fingerprint } => {
            let verifier = Arc::new(FingerprintVerifier::new(fingerprint));

            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth()
        }
        VerifyMode::Tofu { verifier } => {
            let cert_verifier = Arc::new(TofuServerCertVerifier::new(verifier.clone()));

            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(cert_verifier)
                .with_no_client_auth()
        }
        VerifyMode::SkipVerification => {
            let verifier = Arc::new(SkipVerifier);

            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth()
        }
    };

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_server_name_ip_v4_extraction() {
        let ip = rustls::pki_types::IpAddr::from(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)));
        let server_name = ServerName::IpAddress(ip);

        let hostname = match &server_name {
            ServerName::DnsName(name) => name.as_ref().to_string(),
            ServerName::IpAddress(addr) => std::net::IpAddr::from(*addr).to_string(),
            _ => panic!("unexpected server name type"),
        };

        assert_eq!(hostname, "192.168.1.100");
    }

    #[test]
    fn test_server_name_ip_v6_extraction() {
        let ip = rustls::pki_types::IpAddr::from(IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)));
        let server_name = ServerName::IpAddress(ip);

        let hostname = match &server_name {
            ServerName::DnsName(name) => name.as_ref().to_string(),
            ServerName::IpAddress(addr) => std::net::IpAddr::from(*addr).to_string(),
            _ => panic!("unexpected server name type"),
        };

        assert_eq!(hostname, "::1");
    }
}
