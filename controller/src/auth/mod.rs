use actix_web::http::header::HeaderValue;
use serde::Deserialize;
use time::OffsetDateTime;

#[derive(Debug)]
pub enum AuthenticationType {
    #[cfg(feature = "auth-basic")]
    Basic,
    #[cfg(feature = "auth-jwt")]
    Jwt,
}

pub(crate) fn from_authorization_header(
    header: &HeaderValue,
) -> Result<(AuthenticationType, String), ()> {
    let Ok(header) = header.to_str() else {
        return Err(());
    };

    #[cfg(feature = "auth-jwt")]
    if let Some(jwt) = header.strip_prefix("Bearer").map(str::trim) {
        return Ok((AuthenticationType::Jwt, jwt.to_owned()));
    }

    #[cfg(feature = "auth-basic")]
    if let Some(creds) = header.strip_prefix("Basic").map(str::trim) {
        use base64::{prelude::BASE64_STANDARD, Engine};

        let decoded = BASE64_STANDARD.decode(creds).or(Err(()))?;
        let creds = std::str::from_utf8(&decoded)
            .map_err(|_| ())?
            .split(':')
            .next()
            .ok_or(())?;
        return Ok((AuthenticationType::Basic, creds.to_owned()));
    }
    Err(())
}

pub fn authenticate(auth_type: &AuthenticationType, credentials: &str) -> Result<String, Error> {
    match auth_type {
        #[cfg(feature = "auth-basic")]
        AuthenticationType::Basic => basic::authenticate(credentials),
        #[cfg(feature = "auth-jwt")]
        AuthenticationType::Jwt => jwt::authenticate(credentials),
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    #[cfg(feature = "auth-jwt")]
    jwt: jwt::Config,
}

pub fn init(config: &Config) {
    #[cfg(feature = "auth-jwt")]
    jwt::init(&config.jwt);
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Expired at {at}")]
    Expired { at: OffsetDateTime },
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Invalid credentials format")]
    InvalidCredentialsFormat,
    #[error("Missing mandatory field: {field}")]
    MissingField { field: String },
}

#[cfg(feature = "auth-jwt")]
mod jwt;
#[cfg(feature = "auth-basic")]
mod basic {
    use super::Error;

    pub fn authenticate(username: &str) -> Result<String, Error> {
        Ok(username.to_owned())
    }
}
