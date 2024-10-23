use std::{future::Future, pin::Pin, rc::Rc};

use actix_session::SessionExt;
use actix_utils::future::{ready, Ready};
use actix_web::{
    body::MessageBody,
    dev::{forward_ready, Service, ServiceRequest, ServiceResponse, Transform},
    Error, HttpMessage as _, Result,
};

use super::Identity;

#[derive(Default, Clone)]
pub(crate) struct IdentityMiddleware;

impl<S, B> Transform<S, ServiceRequest> for IdentityMiddleware
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Transform = InnerIdentityMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(InnerIdentityMiddleware {
            service: Rc::new(service),
        }))
    }
}

pub(crate) struct InnerIdentityMiddleware<S> {
    service: Rc<S>,
}

impl<S> Clone for InnerIdentityMiddleware<S> {
    fn clone(&self) -> Self {
        Self {
            service: Rc::clone(&self.service),
        }
    }
}

impl<S, B> Service<ServiceRequest> for InnerIdentityMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let srv = Rc::clone(&self.service);
        Box::pin(async move {
            let identity_inner = super::IdentityInner {
                session: req.get_session(),
            };
            req.extensions_mut().insert(identity_inner);
            enforce_policies(&req);
            srv.call(req).await
        })
    }
}

fn enforce_policies(req: &ServiceRequest) {
    let session = match Identity::extract(&req.extensions()) {
        Ok(session) => session,
        Err(_err) => {
            return;
        }
    };

    match identity_expired(&session) {
        Err(_) | Ok(true) => session.logout(),
        _ => (),
    }
}

fn identity_expired(session: &Identity) -> Result<bool, actix_session::SessionGetError> {
    // TODO implement session expiration.
    Ok(false)
}
