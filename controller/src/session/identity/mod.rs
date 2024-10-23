use actix_utils::future::{ready, Ready};
use actix_web::{
    dev::{Extensions, Payload},
    http::StatusCode,
    Error, FromRequest, HttpMessage, HttpRequest, HttpResponse,
};
use uuid::Uuid;

pub struct Identity(IdentityInner);

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
}

impl Identity {
    pub fn login(ext: &Extensions) -> Result<(), actix_session::SessionInsertError> {
        let inner = IdentityInner::extract(ext);

        // TODO: store identity UID.
        inner.session.renew();

        Ok(())
    }

    pub fn logout(self) {
        self.0.session.purge();
    }

    fn extract(ext: &Extensions) -> Result<Self, ExtractError> {
        let inner = IdentityInner::extract(ext);
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
}
