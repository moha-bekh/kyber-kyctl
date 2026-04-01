// Project Kyber: jwt.rs
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

//! JWT payload helpers (no signature verification).

use std::time::Duration;

use base64::Engine;

use crate::Error;

/// Extracts a string claim from a JWT payload without signature verification.
pub fn parse_claim(jwt: &str, claim: &str) -> Result<String, Error> {
    decode_payload(jwt)?[claim]
        .as_str()
        .map(|s| s.to_owned())
        .ok_or_else(|| Error::Jwt(format!("missing '{claim}' claim")))
}

/// Parses the `exp` claim from a JWT without signature verification.
pub fn parse_exp(jwt: &str) -> Result<u64, Error> {
    decode_payload(jwt)?["exp"]
        .as_u64()
        .ok_or_else(|| Error::Jwt("missing or invalid 'exp' claim".to_owned()))
}

/// Returns the duration until 60 seconds before the token expires.
/// Returns `Duration::ZERO` if the token has already (nearly) expired.
pub fn time_until_refresh(exp: u64) -> Duration {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let refresh_at = exp.saturating_sub(60);
    if refresh_at > now {
        Duration::from_secs(refresh_at - now)
    } else {
        Duration::ZERO
    }
}

fn decode_payload(jwt: &str) -> Result<serde_json::Value, Error> {
    let payload_b64 = jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| Error::Jwt("invalid JWT format".to_owned()))?;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|e| Error::Jwt(format!("base64 decode failed: {e}")))?;

    serde_json::from_slice(&decoded).map_err(|e| Error::Jwt(format!("payload parse failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_jwt(payload_json: &str) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        format!("{header}.{payload}.signature")
    }

    #[test]
    fn parse_exp_valid() {
        let jwt = make_jwt(r#"{"sub":"user","exp":9999999999}"#);
        assert_eq!(parse_exp(&jwt).unwrap(), 9999999999);
    }

    #[test]
    fn parse_exp_missing() {
        let jwt = make_jwt(r#"{"sub":"user"}"#);
        assert!(parse_exp(&jwt).is_err());
    }

    #[test]
    fn parse_exp_invalid_format() {
        assert!(parse_exp("onlyone").is_err());
    }

    #[test]
    fn parse_claim_valid() {
        let jwt = make_jwt(r#"{"nonce":"test-value"}"#);
        assert_eq!(parse_claim(&jwt, "nonce").unwrap(), "test-value");
    }

    #[test]
    fn parse_claim_missing() {
        let jwt = make_jwt(r#"{"sub":"user"}"#);
        assert!(parse_claim(&jwt, "nonce").is_err());
    }

    #[test]
    fn refresh_expired() {
        assert_eq!(time_until_refresh(0), Duration::ZERO);
        assert_eq!(time_until_refresh(60), Duration::ZERO);
    }

    #[test]
    fn refresh_future() {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let exp = now + 3600;
        let d = time_until_refresh(exp);
        assert!(d.as_secs() >= 3535 && d.as_secs() <= 3540);
    }
}
