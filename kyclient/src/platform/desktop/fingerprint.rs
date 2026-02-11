// Project Kyber: fingerprint.rs
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

//! Certificate fingerprint calculation and parsing.
//!
//! Fingerprints use the format: sha256:xx:xx:xx:...

use sha2::{Digest, Sha256};

/// Calculate the SHA-256 fingerprint of a DER-encoded certificate.
/// Returns format: sha256:xx:xx:xx:...
pub fn calculate(cert_der: &[u8]) -> String {
    let hash = Sha256::digest(cert_der);
    let hex_parts: Vec<String> = hash.iter().map(|b| format!("{:02x}", b)).collect();
    format!("sha256:{}", hex_parts.join(":"))
}

/// Parse a fingerprint string and return the raw hash bytes.
/// Accepts format: sha256:xx:xx:xx:... or just xx:xx:xx:...
pub fn parse(fingerprint: &str) -> Option<Vec<u8>> {
    let s = fingerprint.trim();
    let hex_part = s.strip_prefix("sha256:").unwrap_or(s);
    let hex_clean: String = hex_part.chars().filter(|c| *c != ':').collect();

    // SHA-256 hash should be 64 hex characters
    if hex_clean.len() != 64 || !hex_clean.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let bytes: Vec<u8> = (0..hex_clean.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex_clean[i..i + 2], 16).ok())
        .collect();

    if bytes.len() == 32 {
        Some(bytes)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_fingerprint() {
        let cert_der = b"test certificate data";
        let fp = calculate(cert_der);
        assert!(fp.starts_with("sha256:"));
        assert_eq!(fp.matches(':').count(), 32); // sha256: + 31 colons between 32 bytes
    }

    #[test]
    fn test_parse_fingerprint() {
        let fp = "sha256:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";
        let bytes = parse(fp).unwrap();
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[0], 0xaa);
        assert_eq!(bytes[1], 0xbb);
    }

    #[test]
    fn test_parse_without_prefix() {
        let fp = "aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99";
        let bytes = parse(fp).unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse("invalid").is_none());
        assert!(parse("sha256:short").is_none());
    }
}
