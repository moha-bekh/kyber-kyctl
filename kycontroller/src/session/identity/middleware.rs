// Project Kyber: middleware.rs
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

//! Identity middleware for session management and expiration enforcement.
//!
//! This module implements actix-web's middleware pattern to provide automatic session
//! handling for all HTTP requests. It runs before route handlers and performs two key tasks:
//!
//! 1. **Session Injection**: Extracts the actix-session `Session` object and wraps it in
//!    `IdentityInner`, storing it in request extensions for later extraction by the
//!    `Identity` FromRequest implementation.
//!
//! 2. **Expiration Enforcement**: Validates session expiration on every request and
//!    automatically purges expired sessions before handlers execute.
//!
//! ## Actix Middleware Pattern
//!
//! Actix uses a two-stage middleware pattern:
//!
//! - **Transform**: Creates middleware instances (called once per worker thread)
//! - **Service**: Processes individual requests (called for each HTTP request)

use std::{future::Future, pin::Pin, rc::Rc};

use actix_session::SessionExt;
use actix_utils::future::{Ready, ready};
use actix_web::{
    Error, HttpMessage as _, Result,
    body::MessageBody,
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
};
use time::OffsetDateTime;

use super::Identity;

/// Identity middleware that manages session state for authentication.
///
/// This middleware must be registered **after** `SessionMiddleware` in the actix app:
///
/// ```rust
/// HttpServer::new(|| {
///     App::new()
///         .wrap(IdentityMiddleware {})  // This middleware
///         .wrap(SessionMiddleware::builder(...).build())  // Must come after
///         // ... routes
/// })
/// ```
///
/// **Order matters**: Middleware wraps in reverse order, so SessionMiddleware runs first.
#[derive(Default, Clone)]
pub(crate) struct IdentityMiddleware;

/// Transform trait implementation - creates middleware instances.
///
/// This is called once per worker thread to create the actual middleware service.
/// The `S` generic parameter represents the next middleware/handler in the chain.
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

    /// Creates a new middleware instance wrapping the next service in the chain.
    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(InnerIdentityMiddleware {
            service: Rc::new(service),
        }))
    }
}

/// The actual middleware service that processes requests.
///
/// This struct holds a reference to the next service in the middleware chain.
/// It's cloned for each request.
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

/// Service trait implementation - processes individual requests.
///
/// This is where the actual middleware logic executes for each HTTP request.
impl<S, B> Service<ServiceRequest> for InnerIdentityMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    S::Future: 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<B>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    // Delegates readiness check to the wrapped service
    forward_ready!(service);

    /// Processes a single HTTP request.
    ///
    /// ## Execution flow:
    ///
    /// 1. Extract the `Session` object from the request (placed by SessionMiddleware)
    /// 2. Wrap it in `IdentityInner` and insert into request extensions
    /// 3. Enforce expiration policies (purge expired sessions)
    /// 4. Call the next service in the chain (other middleware or the handler)
    ///
    /// This runs **before** route handlers, so expired sessions are purged before
    /// handlers can access them.
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

/// Enforces session expiration policies on the current request.
///
/// If a session exists and has expired, it's immediately purged from the cookie.
fn enforce_policies(req: &ServiceRequest) {
    let session = match Identity::extract(&req.extensions()) {
        Ok(session) => session,
        Err(_err) => return,
    };

    if identity_expired(&session) {
        session.logout()
    }
}

/// Checks if a session has expired based on current UTC time.
fn identity_expired(session: &Identity) -> bool {
    let now = OffsetDateTime::now_utc();
    session
        .expiration()
        .map(|opt| opt.map(|exp| now > exp).unwrap_or(false))
        .unwrap_or(false)
}
