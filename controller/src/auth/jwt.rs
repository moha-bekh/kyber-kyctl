use std::{fs::File, io::Read, sync::OnceLock};

use super::Error;
use jsonwebtoken::{
    decode,
    errors::{Error as JWTError, ErrorKind},
    Algorithm, DecodingKey, Validation,
};
use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigKey {
    Plain(String),
    File(std::path::PathBuf),
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    algorithm: Algorithm,
    key: ConfigKey,
}

impl Default for Config {
    fn default() -> Self {
        const DEFAULT_DEV_KEY: &str = "kyber-authentication-development-key";
        Self {
            algorithm: Algorithm::HS256,
            key: ConfigKey::Plain(DEFAULT_DEV_KEY.to_owned()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Claims {
    // Audience, identifies the recipient (likely, the streaming session).
    #[allow(unused)]
    aud: String,
    // Expiration time, UTC timestamp.
    exp: i64,
    // Subject, identifies the owner of the token.
    sub: String,
}

struct BackendContext {
    key: DecodingKey,
    validation: Validation,
}

static BACKEND: OnceLock<BackendContext> = OnceLock::new();

pub fn authenticate(token: &str) -> Result<String, Error> {
    let BackendContext { key, validation } = BACKEND.get().expect("Backend not initialized");

    let jwt = decode::<Claims>(token, key, validation);

    // Special handling: Extract expiration timestamp from expired tokens
    // The jsonwebtoken library doesn't expose the exp claim in the error,
    // so we re-decode without expiration validation to get the timestamp.
    if let Err(e) = &jwt {
        if matches!(e.kind(), ErrorKind::ExpiredSignature) {
            let mut validation = validation.clone();
            validation.validate_exp = false;
            let expired = decode::<Claims>(token, key, &validation)?;
            let at = OffsetDateTime::from_unix_timestamp(expired.claims.exp)
                .or(Err(Error::InvalidCredentialsFormat))?;
            return Err(Error::Expired { at });
        }
    }

    let jwt = jwt?;
    Ok(jwt.claims.sub)
}

fn decode_key(algorithm: Algorithm, key: &[u8]) -> DecodingKey {
    match algorithm {
        Algorithm::HS256 => DecodingKey::from_secret(key),
        Algorithm::RS256 => DecodingKey::from_rsa_pem(key).expect("Invalid RSA PEM"),
        _ => panic!("Unsupported JWT algorithm (currently supported: HS256, RS256)"),
    }
}
pub(super) fn init(config: &Config) {
    let key = match &config.key {
        ConfigKey::Plain(key) => decode_key(config.algorithm, key.as_bytes()),
        ConfigKey::File(path) => {
            let mut key = Vec::new();
            let mut file = File::open(path).expect("Invalid key path");
            file.read_to_end(&mut key)
                .expect("Failed to read key content");
            // Trim leading and trailing whitespace (including newlines) as the jsonwebtoken
            // crate doesn't handle them.
            let key = key.as_slice().trim_ascii();
            decode_key(config.algorithm, key)
        }
    };

    let mut validation = Validation::new(config.algorithm);
    validation.set_required_spec_claims(&["exp", "sub", "aud"]);
    validation.set_audience(&["kyber"]);

    // Set leeway to 0 as there's no leeway to the cookie session expiration in Kyber for now.
    validation.leeway = 0;

    BACKEND
        .set(BackendContext { key, validation })
        .or(Err(()))
        .expect("Cannot call init() twice");
    log::info!("Backend initialized with algorithm {:?}", config.algorithm);
}

impl From<JWTError> for Error {
    fn from(from: JWTError) -> Self {
        match from.into_kind() {
            ErrorKind::InvalidSignature => Error::InvalidSignature,
            ErrorKind::MissingRequiredClaim(claim) => Error::MissingField { field: claim },
            _ => Error::InvalidCredentialsFormat,
        }
    }
}
