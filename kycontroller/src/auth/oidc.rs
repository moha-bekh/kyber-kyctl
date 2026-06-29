// Project Kyber: oidc.rs
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

//! OIDC authentication backend.
//!
//! Validates access tokens from external OIDC providers using JWKS-based
//! signature verification. The key set is refreshed by a background task
//! on a configurable interval; an unknown `kid` triggers an immediate
//! out-of-band refresh so the next authentication attempt succeeds.
//!
//! ## Configuration
//!
//! ```toml
//! [kycontroller.auth.oidc]
//! enabled = true
//! issuer = "https://gitlab.com"
//! client_id = "your-client-id"
//! identity_claim = "sub"           # or "email" / "preferred_username"
//! allowed_identifiers = ["user1@example.com"]  # identity-based ACL
//! allowed_groups = ["friends"]                  # group-based ACL
//! # audience = "<machine-client-id>"          # optional: the `aud` a token must carry
//! ```
//!
//! Access is granted when the user matches `allowed_identifiers` **or** belongs
//! to a group in `allowed_groups`. If both lists are empty, all authenticated
//! users are allowed. The `groups` claim must be present in the access token
//! (configure a "groups" client scope in the IdP).

use std::sync::{Arc, RwLock};
use std::time::Duration;

use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{DecodingKey, Validation};
use log::{debug, info, warn};
use rustls_platform_verifier::ConfigVerifierExt;
use serde::Deserialize;
use time::OffsetDateTime;
use tokio::sync::Notify;

use crate::auth::{Backend, Error, Session};

/// Which JWT claim identifies the user.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityClaim {
    #[default]
    Sub,
    Email,
    PreferredUsername,
}

const DEFAULT_JWKS_CACHE_TTL_SECS: u64 = 300;

/// OIDC backend configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    /// OIDC issuer URL (e.g. `https://gitlab.com`).
    pub issuer: String,
    /// OAuth2 client ID registered with the provider.
    pub client_id: String,
    /// JWT claim used to identify the user.
    pub identity_claim: IdentityClaim,
    /// If non-empty, only these identities are allowed in.
    pub allowed_identifiers: Vec<String>,
    /// If non-empty, only users belonging to one of these groups are allowed.
    /// Requires a `groups` claim in the access token (IdP client scope).
    pub allowed_groups: Vec<String>,
    /// How long (in seconds) the JWKS key set is cached before re-fetching.
    pub jwks_cache_ttl: Option<u64>,
    /// If set, the JWT `aud` claim must match. Unset: `aud` is not checked.
    pub audience: Option<String>,
}

/// Shared JWKS key pool refreshed by a background task.
struct JwksPool {
    keys: Arc<RwLock<jsonwebtoken::jwk::JwkSet>>,
    /// Notify the background task to perform an immediate refresh
    /// (e.g. on `kid` miss during key rotation).
    refresh_notify: Arc<Notify>,
}

impl JwksPool {
    async fn new(issuer: String, ttl: Duration) -> Result<Self, String> {
        let tls_config = Arc::new(
            rustls::ClientConfig::with_platform_verifier()
                .map_err(|e| format!("TLS platform verifier error: {e}"))?,
        );
        let client = awc::Client::builder()
            .connector(awc::Connector::new().rustls_0_23(tls_config))
            .timeout(Duration::from_secs(10))
            .finish();

        let initial = fetch_jwks(&client, &issuer).await?;
        info!("JWKS keys loaded for {issuer}");

        let keys = Arc::new(RwLock::new(initial));
        let refresh_notify = Arc::new(Notify::new());

        let task_keys = Arc::clone(&keys);
        let task_notify = Arc::clone(&refresh_notify);

        tokio::task::spawn_local(async move {
            loop {
                // Sleep until TTL expires or an immediate refresh is requested.
                if tokio::time::timeout(ttl, task_notify.notified())
                    .await
                    .is_ok()
                {
                    debug!("JWKS immediate refresh requested");
                }

                match fetch_jwks(&client, &issuer).await {
                    Ok(jwks) => {
                        *task_keys.write().expect("JWKS lock poisoned") = jwks;
                        debug!("JWKS keys refreshed for {issuer}");
                    }
                    Err(e) => {
                        warn!("JWKS refresh failed: {e}");
                    }
                }
            }
        });

        Ok(Self {
            keys,
            refresh_notify,
        })
    }

    fn get(&self) -> jsonwebtoken::jwk::JwkSet {
        self.keys.read().expect("JWKS lock poisoned").clone()
    }

    fn request_refresh(&self) {
        self.refresh_notify.notify_one();
    }
}

pub struct Oidc {
    config: Config,
    jwks_pool: JwksPool,
}

impl Oidc {
    pub async fn new(config: &Config) -> Result<Self, String> {
        if !config.issuer.starts_with("https://") {
            return Err(format!(
                "OIDC issuer must use HTTPS (got \"{}\")",
                config.issuer
            ));
        }

        let config = normalize(config)?;

        let jwks_cache_ttl =
            Duration::from_secs(config.jwks_cache_ttl.unwrap_or(DEFAULT_JWKS_CACHE_TTL_SECS));

        let jwks_pool = JwksPool::new(config.issuer.clone(), jwks_cache_ttl).await?;
        info!("OIDC backend initialized for issuer: {}", config.issuer);

        Ok(Self { jwks_pool, config })
    }

    pub fn www_authenticate_value(&self) -> String {
        format!(
            "Bearer realm=\"oidc\", issuer=\"{}\", client_id=\"{}\"",
            self.config.issuer.replace('"', "\\\""),
            self.config.client_id.replace('"', "\\\""),
        )
    }
}

#[derive(Deserialize)]
struct ProviderMetadata {
    issuer: String,
    jwks_uri: String,
}

async fn fetch_jwks(
    client: &awc::Client,
    issuer: &str,
) -> Result<jsonwebtoken::jwk::JwkSet, String> {
    let issuer = issuer.trim_end_matches('/');
    let config_url = format!("{issuer}/.well-known/openid-configuration");

    let metadata: ProviderMetadata = client
        .get(&config_url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch OpenID configuration from {config_url}: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenID configuration: {e}"))?;

    // OIDC Discovery 4.3: the issuer in the metadata MUST match the URL used to fetch it.
    if metadata.issuer.trim_end_matches('/') != issuer {
        return Err(format!(
            "Issuer mismatch: expected \"{issuer}\", got \"{}\"",
            metadata.issuer
        ));
    }

    if !metadata.jwks_uri.starts_with("https://") {
        return Err(format!(
            "Provider returned a non-HTTPS jwks_uri (\"{}\")",
            metadata.jwks_uri
        ));
    }

    let jwks: jsonwebtoken::jwk::JwkSet = client
        .get(&metadata.jwks_uri)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch JWKS from {}: {e}", metadata.jwks_uri))?
        .json()
        .await
        .map_err(|e| format!("Failed to parse JWKS: {e}"))?;

    Ok(jwks)
}

#[derive(Deserialize)]
struct Claims {
    exp: i64,
    // Access tokens are not required to carry `sub` (some IdPs strip it
    // from access tokens via mappers). Only enforce its presence when
    // `identity_claim = "sub"`.
    sub: Option<String>,
    email: Option<String>,
    preferred_username: Option<String>,
    /// Group memberships from the IdP (requires a `groups` client scope).
    #[serde(default)]
    groups: Vec<String>,
}

impl std::fmt::Debug for Claims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claims")
            .field("exp", &self.exp)
            .field("sub", &self.sub.as_ref().map(|_| "[REDACTED]"))
            .field("email", &self.email.as_ref().map(|_| "[REDACTED]"))
            .field(
                "preferred_username",
                &self.preferred_username.as_ref().map(|_| "[REDACTED]"),
            )
            .field("groups", &self.groups)
            .finish()
    }
}

impl Backend for Oidc {
    fn authenticate(&self, token: &str) -> Result<Session, Error> {
        let header = jsonwebtoken::decode_header(token).map_err(|e| {
            debug!("JWT header decode failed: {e}");
            Error::InvalidCredentialsFormat
        })?;

        let kid = header.kid.ok_or_else(|| Error::MissingField {
            field: "kid".to_owned(),
        })?;

        let jwks = self.jwks_pool.get();

        let jwk = match jwks.find(&kid) {
            Some(jwk) => jwk,
            None => {
                // Unknown kid — likely key rotation. Request a background
                // refresh so the next attempt succeeds.
                self.jwks_pool.request_refresh();
                return Err(Error::OidcUnavailable {
                    reason: format!("unknown kid \"{kid}\", JWKS refresh requested"),
                });
            }
        };

        let decoding_key = DecodingKey::from_jwk(jwk).map_err(|e| {
            debug!("JWK decode failed: {e}");
            Error::InvalidSignature
        })?;

        // Use the algorithm declared in the JWK, not the JWT header, to
        // prevent an attacker from forcing a weaker algorithm (e.g. HS256
        // with a public RSA key as the HMAC secret). Only asymmetric
        // algorithms are accepted; symmetric (HMAC) algorithms are
        // rejected because OIDC providers sign with private keys.
        use jsonwebtoken::Algorithm;
        const ALLOWED_ALGS: [Algorithm; 5] = [
            Algorithm::RS256,
            Algorithm::RS384,
            Algorithm::RS512,
            Algorithm::ES256,
            Algorithm::ES384,
        ];
        let alg = jwk
            .common
            .key_algorithm
            .and_then(|a| a.to_string().parse::<Algorithm>().ok())
            .or_else(|| infer_alg_from_kty(&jwk.algorithm))
            .filter(|a| ALLOWED_ALGS.contains(a))
            .ok_or(Error::InvalidSignature)?;
        let mut validation = Validation::new(alg);
        if let Some(audience) = &self.config.audience {
            validation.set_audience(std::slice::from_ref(audience));
            // Without this, a token with no `aud` passes.
            validation.set_required_spec_claims(&["exp", "aud"]);
        } else {
            // Access tokens (RFC 6750) have a provider-specific `aud`.
            validation.validate_aud = false;
        }
        validation.set_issuer(&[&self.config.issuer]);

        let jwt = jsonwebtoken::decode::<Claims>(token, &decoding_key, &validation);

        if let Err(e) = &jwt
            && matches!(e.kind(), ErrorKind::ExpiredSignature)
        {
            let mut v = validation.clone();
            v.validate_exp = false;
            let expired =
                jsonwebtoken::decode::<Claims>(token, &decoding_key, &v).map_err(map_jwt_error)?;
            let at = OffsetDateTime::from_unix_timestamp(expired.claims.exp)
                .map_err(|_| Error::InvalidCredentialsFormat)?;
            return Err(Error::Expired { at });
        }

        let claims = jwt.map_err(map_jwt_error)?.claims;

        let identity = match self.config.identity_claim {
            IdentityClaim::Sub => claims.sub.ok_or_else(|| Error::MissingField {
                field: "sub".to_owned(),
            })?,
            IdentityClaim::Email => claims.email.ok_or_else(|| Error::MissingField {
                field: "email".to_owned(),
            })?,
            IdentityClaim::PreferredUsername => {
                claims
                    .preferred_username
                    .ok_or_else(|| Error::MissingField {
                        field: "preferred_username".to_owned(),
                    })?
            }
        };

        let identity_match = !self.config.allowed_identifiers.is_empty()
            && self.config.allowed_identifiers.contains(&identity);

        let group_match = !self.config.allowed_groups.is_empty()
            && claims
                .groups
                .iter()
                .any(|g| self.config.allowed_groups.contains(g));

        let allow_all =
            self.config.allowed_identifiers.is_empty() && self.config.allowed_groups.is_empty();

        if !identity_match && !group_match && !allow_all {
            return Err(Error::NotAuthorized { identity });
        }

        let exp = OffsetDateTime::from_unix_timestamp(claims.exp)
            .map_err(|_| Error::InvalidCredentialsFormat)?;

        Ok(Session {
            username: identity,
            expiration: Some(exp),
        })
    }
}

fn infer_alg_from_kty(
    params: &jsonwebtoken::jwk::AlgorithmParameters,
) -> Option<jsonwebtoken::Algorithm> {
    use jsonwebtoken::Algorithm;
    use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve};
    match params {
        AlgorithmParameters::RSA(_) => Some(Algorithm::RS256),
        AlgorithmParameters::EllipticCurve(ec) => match ec.curve {
            EllipticCurve::P256 => Some(Algorithm::ES256),
            EllipticCurve::P384 => Some(Algorithm::ES384),
            _ => None,
        },
        _ => None,
    }
}

fn map_jwt_error(e: jsonwebtoken::errors::Error) -> Error {
    use jsonwebtoken::errors::ErrorKind;
    debug!("JWT validation error: {e}");
    match e.kind() {
        ErrorKind::InvalidSignature
        | ErrorKind::InvalidEcdsaKey
        | ErrorKind::InvalidRsaKey(_)
        | ErrorKind::InvalidKeyFormat
        | ErrorKind::InvalidAlgorithm
        | ErrorKind::InvalidAlgorithmName => Error::InvalidSignature,
        ErrorKind::InvalidAudience => Error::InvalidAudience,
        ErrorKind::MissingRequiredClaim(claim) => Error::MissingField {
            field: claim.clone(),
        },
        _ => Error::InvalidCredentialsFormat,
    }
}

#[cfg(test)]
impl JwksPool {
    fn new_with_keys(jwks: jsonwebtoken::jwk::JwkSet) -> Self {
        Self {
            keys: Arc::new(RwLock::new(jwks)),
            refresh_notify: Arc::new(Notify::new()),
        }
    }
}

#[cfg(test)]
impl Oidc {
    fn new_for_test(config: Config, jwks: jsonwebtoken::jwk::JwkSet) -> Self {
        Self {
            jwks_pool: JwksPool::new_with_keys(jwks),
            config: normalize(&config).expect("valid test config"),
        }
    }
}

/// Normalize the config once, so every user of it sees the same values.
fn normalize(config: &Config) -> Result<Config, String> {
    let mut config = config.clone();
    config.issuer = config.issuer.trim_end_matches('/').to_owned();
    config.audience = config.audience.map(|a| a.trim().to_owned());
    // An empty audience would reject every token.
    if config.audience.as_deref() == Some("") {
        return Err("OIDC audience must not be empty when set".to_owned());
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    // RSA-2048 test key (generated offline, never used in production).
    const TEST_PRIVATE_KEY_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEAtuaxWBRFKKe9GU6vq0DgLhJK9b0Dx13ARkZJk8tnctbsSyrZ
K393J5s9TTGiFPUd3QiBXp62+9ld2zNnNRX7bOX56RvRyLdS1LilFNfPay9+t+QM
aluiSUYtleTSkBy1BLSrkq9ZXNbbF0g4LL5RmCNGj4HXFBLQMRbr8CPEh4BPYSwG
SaOIWavry5LdJ4N5LDhF4zWqNzwcKa4aVunqOwldLsPh+HU5rNGN7rwiELUjK0J2
67KEbiq3loPBntaSCQ80/Qf7ln0FR9nd4AY/ZxE8plvcQpY/a57zhFWD+7Bc/f3T
bIEdbEqiJsWynuXJw0n8ziB4u/OnGL9PJ/REOwIDAQABAoIBABbo5JPkzhjekegm
nAh2ct4yemM3js6dJqVmznaNxuJym1x6/ILfT3yNjGsr6WJ4pYn3M8U3WjJHsuLv
GGFk5H5qfBg/3NIobF6q1jq7IYcKhvPfw9JasgQ9nx4kcF+OyPnS657PCgGABCdx
a1/nYu8MlB0LpXYlOSC8/0awJEDYkl0Ynund+iqnakLXFlEeWucPpuhSFFwc5kkS
IFPaqYbLCOySn3qklEqEFy3tvDITovyKSAw/0RqY/qDVSqxJHIPgj2WBcT4OZ4YO
/vVOhvCgLZiYd5fV4rHOUcXgNmIP6t0yKbaSaJYpfV/KDrlDxEisX1rWKUaaYsRb
vO8p9dECgYEA3wZfTc9IZooea0orxmFzB6+/vLt7t+IE2TrcIjeJOc7xyTsHZHHk
q45fPNHRbsdbI7NBm7H+jWnzwanVrc9J7Pxr7SasNN6FjzHJuYpoA0/0m+Pg3NC1
jAuL7gJP9ph57j5VI0bx8SnLl2O+RDvFlcAaYhhvU787BkEcLzNzmGkCgYEA0fGc
ueWy+t8wipIDem95EbwCWDH0nbigX+eqeeqU0a7HqKveSycywHruIcXpmt/TZnfe
yJ9XDYztix7MF9DNtBS+X0FkNEHJEhd7O079308t5c+bhKivwSM7O4PEvb9TGBzH
AMTZk3cae+SyF56kqUFhUBOrF03gONjuZaBGQwMCgYBgcE33xIFHLcR3mmwRbYQj
N8EOaHfYDNkR4hzcgu1I3Q66eFb6/TDnajoOf6AzaeskJdhh7fbnyYAJJ1ppvS/3
VTOKKeXfOrXANMRJAet2OzXHzyMl+O34L+oDMmbHeCmJdtSz23K1WOBbh1r4AGyx
E4+VCYHiUdfBHZiFZJdAuQKBgEuOats0YZ+L54pXVk0gBTBuu2FGSUTiB0W8uEu8
MubrwgtgH1ZouwkutLkO4y3L0+cSyOjm/4tu5qak5MrFaOKgfIsHm6p66oRKUx1b
Jl7RvjDw8IVjuC0IIu8eUxX+30bURIJwVeWOfceM3NNHUvdfAkixa2etrT6IQ/Pr
JJ5FAoGAbub5vM4p4dP5rY/Fk9zU4uRNtD3DUK+Gs/ETHjgVJ+z5Lb+S9aMLNr8t
d2r+ZEFdsUB5GJankyfArTMOK05UjC8PIbUer0iiawto/qXmCzepclrp1Ap0DZv1
yQX6iM2kKs7vI3nTID1fVSDu6MOqsaPWbVtWnl37M8P56c57zyU=
-----END RSA PRIVATE KEY-----";

    const TEST_JWKS_JSON: &str = r#"{"keys":[{"kty":"RSA","kid":"test-key-1","use":"sig","alg":"RS256","n":"tuaxWBRFKKe9GU6vq0DgLhJK9b0Dx13ARkZJk8tnctbsSyrZK393J5s9TTGiFPUd3QiBXp62-9ld2zNnNRX7bOX56RvRyLdS1LilFNfPay9-t-QMaluiSUYtleTSkBy1BLSrkq9ZXNbbF0g4LL5RmCNGj4HXFBLQMRbr8CPEh4BPYSwGSaOIWavry5LdJ4N5LDhF4zWqNzwcKa4aVunqOwldLsPh-HU5rNGN7rwiELUjK0J267KEbiq3loPBntaSCQ80_Qf7ln0FR9nd4AY_ZxE8plvcQpY_a57zhFWD-7Bc_f3TbIEdbEqiJsWynuXJw0n8ziB4u_OnGL9PJ_REOw","e":"AQAB"}]}"#;

    const TEST_ISSUER: &str = "https://idp.example.com";

    fn test_jwks() -> jsonwebtoken::jwk::JwkSet {
        serde_json::from_str(TEST_JWKS_JSON).expect("static JWKS JSON is valid")
    }

    fn default_config() -> Config {
        Config {
            issuer: TEST_ISSUER.to_owned(),
            client_id: "kyber".to_owned(),
            identity_claim: IdentityClaim::Sub,
            allowed_identifiers: vec![],
            allowed_groups: vec![],
            jwks_cache_ttl: None,
            audience: None,
        }
    }

    fn default_oidc() -> Oidc {
        Oidc::new_for_test(default_config(), test_jwks())
    }

    #[derive(serde::Serialize)]
    struct TokenClaims {
        #[serde(skip_serializing_if = "Option::is_none")]
        sub: Option<String>,
        iss: String,
        exp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        email: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        preferred_username: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        groups: Vec<String>,
    }

    fn make_token(claims: &TokenClaims, kid: Option<&str>) -> String {
        let encoding_key = jsonwebtoken::EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY_PEM.as_bytes())
            .expect("test private key is valid RSA PEM");
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = kid.map(str::to_owned);
        jsonwebtoken::encode(&header, claims, &encoding_key).expect("token encoding failed")
    }

    fn future_exp() -> i64 {
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600) as i64
    }

    fn past_exp() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .saturating_sub(3600) as i64
    }

    fn valid_claims() -> TokenClaims {
        TokenClaims {
            sub: Some("user-123".to_owned()),
            iss: TEST_ISSUER.to_owned(),
            exp: future_exp(),
            email: None,
            preferred_username: None,
            groups: vec![],
        }
    }

    #[test]
    fn valid_token_sub_claim() {
        let token = make_token(&valid_claims(), Some("test-key-1"));
        let session = default_oidc().authenticate(&token).unwrap();
        assert_eq!(session.username, "user-123");
        assert!(session.expiration.is_some());
    }

    #[test]
    fn valid_token_email_claim() {
        let oidc = Oidc::new_for_test(
            Config {
                identity_claim: IdentityClaim::Email,
                ..default_config()
            },
            test_jwks(),
        );
        let claims = TokenClaims {
            email: Some("alice@example.com".to_owned()),
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let session = oidc.authenticate(&token).unwrap();
        assert_eq!(session.username, "alice@example.com");
    }

    #[test]
    fn email_claim_works_when_sub_missing() {
        // Some IdPs (e.g. Keycloak realms with a custom mapper) strip `sub`
        // from access tokens. The controller must still authenticate when
        // `identity_claim = "email"` and `email` is present.
        let oidc = Oidc::new_for_test(
            Config {
                identity_claim: IdentityClaim::Email,
                ..default_config()
            },
            test_jwks(),
        );
        let claims = TokenClaims {
            sub: None,
            email: Some("alice@example.com".to_owned()),
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let session = oidc.authenticate(&token).unwrap();
        assert_eq!(session.username, "alice@example.com");
    }

    #[test]
    fn missing_sub_with_sub_identity_claim_fails() {
        let claims = TokenClaims {
            sub: None,
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let err = default_oidc().authenticate(&token).unwrap_err();
        assert!(matches!(err, Error::MissingField { field } if field == "sub"));
    }

    #[test]
    fn valid_token_preferred_username_claim() {
        let oidc = Oidc::new_for_test(
            Config {
                identity_claim: IdentityClaim::PreferredUsername,
                ..default_config()
            },
            test_jwks(),
        );
        let claims = TokenClaims {
            preferred_username: Some("alice".to_owned()),
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let session = oidc.authenticate(&token).unwrap();
        assert_eq!(session.username, "alice");
    }

    #[test]
    fn expired_token_returns_expired_error() {
        let claims = TokenClaims {
            exp: past_exp(),
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let err = default_oidc().authenticate(&token).unwrap_err();
        assert!(
            matches!(err, Error::Expired { .. }),
            "expected Expired, got {err:?}"
        );
    }

    #[test]
    fn unknown_kid_returns_unavailable() {
        let token = make_token(&valid_claims(), Some("unknown-key-99"));
        let err = default_oidc().authenticate(&token).unwrap_err();
        assert!(
            matches!(&err, Error::OidcUnavailable { reason } if reason.contains("unknown kid")),
            "expected OIDCUnavailable with kid message, got {err:?}"
        );
    }

    #[test]
    fn empty_jwks_returns_unavailable() {
        let empty: jsonwebtoken::jwk::JwkSet = serde_json::from_str(r#"{"keys":[]}"#).unwrap();
        let oidc = Oidc::new_for_test(default_config(), empty);
        let token = make_token(&valid_claims(), Some("test-key-1"));
        let err = oidc.authenticate(&token).unwrap_err();
        assert!(
            matches!(&err, Error::OidcUnavailable { reason } if reason.contains("unknown kid")),
            "expected OIDCUnavailable for empty key set, got {err:?}"
        );
    }

    #[test]
    fn missing_kid_header_returns_missing_field() {
        let token = make_token(&valid_claims(), None);
        let err = default_oidc().authenticate(&token).unwrap_err();
        assert!(
            matches!(&err, Error::MissingField { field } if field == "kid"),
            "expected MissingField(kid), got {err:?}"
        );
    }

    #[test]
    fn wrong_issuer_returns_invalid_format() {
        let claims = TokenClaims {
            iss: "https://evil.example.com".to_owned(),
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let err = default_oidc().authenticate(&token).unwrap_err();
        assert!(
            matches!(err, Error::InvalidCredentialsFormat),
            "expected InvalidCredentialsFormat for wrong issuer, got {err:?}"
        );
    }

    #[test]
    fn acl_identity_match_allows_user() {
        let oidc = Oidc::new_for_test(
            Config {
                allowed_identifiers: vec!["user-123".to_owned()],
                ..default_config()
            },
            test_jwks(),
        );
        let token = make_token(&valid_claims(), Some("test-key-1"));
        assert!(oidc.authenticate(&token).is_ok());
    }

    #[test]
    fn acl_identity_mismatch_denies_user() {
        let oidc = Oidc::new_for_test(
            Config {
                allowed_identifiers: vec!["other-user".to_owned()],
                ..default_config()
            },
            test_jwks(),
        );
        let token = make_token(&valid_claims(), Some("test-key-1"));
        let err = oidc.authenticate(&token).unwrap_err();
        assert!(
            matches!(&err, Error::NotAuthorized { identity } if identity == "user-123"),
            "expected NotAuthorized, got {err:?}"
        );
    }

    #[test]
    fn acl_group_match_allows_user() {
        let oidc = Oidc::new_for_test(
            Config {
                allowed_groups: vec!["friends".to_owned()],
                ..default_config()
            },
            test_jwks(),
        );
        let claims = TokenClaims {
            groups: vec!["friends".to_owned(), "others".to_owned()],
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        assert!(oidc.authenticate(&token).is_ok());
    }

    #[test]
    fn acl_group_mismatch_denies_user() {
        let oidc = Oidc::new_for_test(
            Config {
                allowed_groups: vec!["admins".to_owned()],
                ..default_config()
            },
            test_jwks(),
        );
        let claims = TokenClaims {
            groups: vec!["users".to_owned()],
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        let err = oidc.authenticate(&token).unwrap_err();
        assert!(
            matches!(err, Error::NotAuthorized { .. }),
            "expected NotAuthorized, got {err:?}"
        );
    }

    #[test]
    fn acl_allow_all_when_both_lists_empty() {
        let token = make_token(&valid_claims(), Some("test-key-1"));
        assert!(default_oidc().authenticate(&token).is_ok());
    }

    #[test]
    fn acl_identity_match_when_group_list_also_set() {
        let oidc = Oidc::new_for_test(
            Config {
                allowed_identifiers: vec!["user-123".to_owned()],
                allowed_groups: vec!["admins".to_owned()],
                ..default_config()
            },
            test_jwks(),
        );
        // user-123 is in allowed_identifiers but NOT in allowed_groups.
        // OR semantics: identity match alone is sufficient.
        let token = make_token(&valid_claims(), Some("test-key-1"));
        assert!(oidc.authenticate(&token).is_ok());
    }

    #[test]
    fn acl_group_match_when_identity_list_also_set() {
        let oidc = Oidc::new_for_test(
            Config {
                allowed_identifiers: vec!["other-user".to_owned()],
                allowed_groups: vec!["friends".to_owned()],
                ..default_config()
            },
            test_jwks(),
        );
        // user-123 is NOT in allowed_identifiers but IS in allowed_groups.
        // OR semantics: group match alone is sufficient.
        let claims = TokenClaims {
            groups: vec!["friends".to_owned()],
            ..valid_claims()
        };
        let token = make_token(&claims, Some("test-key-1"));
        assert!(oidc.authenticate(&token).is_ok());
    }

    #[test]
    fn jwk_without_alg_field_infers_from_kty() {
        // RFC 7517 makes `alg` optional; infer RS256 from kty=RSA when absent.
        let jwks_no_alg: jsonwebtoken::jwk::JwkSet = serde_json::from_str(&format!(
            r#"{{"keys":[{{"kty":"RSA","kid":"test-key-1","use":"sig",
                "n":"{}","e":"AQAB"}}]}}"#,
            "tuaxWBRFKKe9GU6vq0DgLhJK9b0Dx13ARkZJk8tnctbsSyrZK393J5s9TTGiFPUd3QiBXp62-9ld2zNnNRX7bOX56RvRyLdS1LilFNfPay9-t-QMaluiSUYtleTSkBy1BLSrkq9ZXNbbF0g4LL5RmCNGj4HXFBLQMRbr8CPEh4BPYSwGSaOIWavry5LdJ4N5LDhF4zWqNzwcKa4aVunqOwldLsPh-HU5rNGN7rwiELUjK0J267KEbiq3loPBntaSCQ80_Qf7ln0FR9nd4AY_ZxE8plvcQpY_a57zhFWD-7Bc_f3TbIEdbEqiJsWynuXJw0n8ziB4u_OnGL9PJ_REOw"
        )).unwrap();
        let oidc = Oidc::new_for_test(default_config(), jwks_no_alg);
        let token = make_token(&valid_claims(), Some("test-key-1"));
        let session = oidc.authenticate(&token).unwrap();
        assert_eq!(session.username, "user-123");
    }

    #[test]
    fn hmac_algorithm_in_jwk_is_rejected() {
        // A JWK advertising HS256 must be rejected regardless of the token signature.
        // This guards against the public-key-as-HMAC-secret attack where an attacker
        // creates a token with alg:HS256 signed using the server's RSA public key.
        let hmac_jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_str(
            r#"{
            "keys":[{
                "kty":"oct","kid":"test-key-1","use":"sig","alg":"HS256",
                "k":"c2VjcmV0"
            }]
        }"#,
        )
        .unwrap();
        let oidc = Oidc::new_for_test(default_config(), hmac_jwks);

        let hmac_key = jsonwebtoken::EncodingKey::from_secret(b"secret");
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
        header.kid = Some("test-key-1".to_owned());
        let token = jsonwebtoken::encode(&header, &valid_claims(), &hmac_key)
            .expect("HS256 token encoding failed");

        let err = oidc.authenticate(&token).unwrap_err();
        assert!(
            matches!(err, Error::InvalidSignature),
            "expected InvalidSignature for HMAC JWK, got {err:?}"
        );
    }

    #[test]
    fn trailing_slash_in_issuer_config_is_normalized() {
        // A token with iss without trailing slash must validate against a config
        // that was provided with one, proving normalization is applied.
        let oidc = Oidc::new_for_test(
            Config {
                issuer: format!("{}/", TEST_ISSUER),
                ..default_config()
            },
            test_jwks(),
        );
        let token = make_token(&valid_claims(), Some("test-key-1"));
        assert!(oidc.authenticate(&token).is_ok());
    }
    #[tokio::test]
    async fn https_required_for_issuer() {
        let result = Oidc::new(&Config {
            issuer: "http://insecure.example.com".to_owned(),
            ..default_config()
        })
        .await;
        match result {
            Ok(_) => panic!("expected Err for http:// issuer"),
            Err(e) => assert!(e.contains("HTTPS"), "error should mention HTTPS, got: {e}"),
        }
    }
}
