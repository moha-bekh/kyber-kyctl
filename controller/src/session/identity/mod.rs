use actix_utils::future::{ready, Ready};
use actix_web::{
    dev::{Extensions, Payload},
    http::StatusCode,
    Error, FromRequest, HttpMessage, HttpRequest, HttpResponse,
};
use time::OffsetDateTime;
use uuid::Uuid;

pub struct Identity(IdentityInner);

const UID_KEY: &str = "uid";
const EXP_KEY: &str = "exp";

#[derive(Clone)]
struct IdentityInner {
    session: actix_session::Session,
}

impl IdentityInner {
    fn extract(ext: &Extensions) -> Self {
        ext.get::<Self>()
            .expect("Actix session middleware is required before identity middleware.")
            .to_owned()
    }

    fn get_identity(&self) -> Result<Uuid, ExtractError> {
        self.session
            .get(UID_KEY)?
            .ok_or_else(|| ExtractError::MissingIdentity)
    }

    fn get_unix_expiration(&self) -> Result<Option<i64>, ExtractError> {
        Ok(self.session.get(EXP_KEY)?)
    }
}

impl Identity {
    pub fn login(
        exp: Option<OffsetDateTime>,
        ext: &Extensions,
    ) -> Result<Uuid, actix_session::SessionInsertError> {
        let inner = IdentityInner::extract(ext);

        let uuid = Uuid::new_v4();
        inner.session.insert(UID_KEY, uuid)?;

        if let Some(exp) = exp {
            inner.session.insert(EXP_KEY, exp.unix_timestamp())?;
        }

        inner.session.renew();

        Ok(uuid)
    }

    pub fn logout(self) {
        self.0.session.purge();
    }

    pub fn uid(&self) -> Result<Uuid, ExtractError> {
        self.0.get_identity()
    }

    pub fn expiration(&self) -> Result<Option<OffsetDateTime>, ExtractError> {
        self.0
            .get_unix_expiration()?
            .map(OffsetDateTime::from_unix_timestamp)
            .transpose()
            .or(Err(ExtractError::InvalidExpirationTime))
    }

    fn extract(ext: &Extensions) -> Result<Self, ExtractError> {
        let inner = IdentityInner::extract(ext);
        inner.get_identity()?;
        Ok(Self(inner))
    }
}

impl FromRequest for Identity {
    type Error = Error;
    type Future = Ready<Result<Self, Self::Error>>;

    #[inline]
    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(Identity::extract(&req.extensions()).map_err(|err| {
            let res = actix_web::error::InternalError::from_response(
                err,
                HttpResponse::new(StatusCode::UNAUTHORIZED),
            );

            actix_web::Error::from(res)
        }))
    }
}

pub mod middleware;

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("Actix session failed: {_0}")]
    SessionError(#[from] actix_session::SessionGetError),
    #[error("Identity not found in the session cookie")]
    MissingIdentity,
    #[error("Expiration time is invalid")]
    InvalidExpirationTime,
}
