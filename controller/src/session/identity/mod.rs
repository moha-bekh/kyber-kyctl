// Project Kyber: mod.rs
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

use actix_utils::future::{ready, Ready};
use actix_web::{
    dev::{Extensions, Payload},
    http::StatusCode,
    Error, FromRequest, HttpMessage, HttpRequest, HttpResponse,
};
use time::OffsetDateTime;
use uuid::Uuid;

/// User identity extracted from session cookies.
///
/// This struct integrates with actix-web's request handling through the `FromRequest` trait,
/// allowing it to be used as a parameter in handler functions for automatic authentication:
///
/// ```rust
/// #[post("/protected")]
/// async fn endpoint(user: Identity) -> HttpResponse {
///     let uid = user.uid().unwrap();
///     // ... authenticated endpoint logic
/// }
/// ```
///
/// The actual cookie is managed by `SessionMiddleware`.
pub struct Identity(IdentityInner);

/// Session cookie keys
const UID_KEY: &str = "uid";
const EXP_KEY: &str = "exp";

/// Internal wrapper around actix-session's Session.
///
/// This is separated from `Identity` to allow cloning and insertion into request extensions.
/// It's stored in extensions by the middleware and retrieved during `FromRequest` extraction.
#[derive(Clone)]
struct IdentityInner {
    session: actix_session::Session,
}

impl IdentityInner {
    /// Extracts the IdentityInner from request extensions.
    fn extract(ext: &Extensions) -> Self {
        // Panic if the Session Middleware was not registered in the actix app,
        // as this indicates a programming error in the middleware setup.
        ext.get::<Self>()
            .expect("Actix session middleware is required before identity middleware.")
            .to_owned()
    }

    /// Retrieves the user ID from the session cookie.
    fn get_identity(&self) -> Result<Uuid, ExtractError> {
        self.session
            .get(UID_KEY)?
            .ok_or_else(|| ExtractError::MissingIdentity)
    }

    /// Retrieves the session expiration timestamp from the cookie.
    ///
    /// Returns `None` if no expiration is set.
    fn get_unix_expiration(&self) -> Result<Option<i64>, ExtractError> {
        Ok(self.session.get(EXP_KEY)?)
    }
}

impl Identity {
    /// Creates a new authenticated session for a user.
    ///
    /// This is called after successful authentication to:
    /// 1. Generate a new UUID v4 for the user
    /// 2. Store the UUID and optional expiration in the session cookie
    /// 3. Renew the session by generating a new cookie
    ///
    /// # Parameters
    /// - `exp`: Session expiration time
    /// - `ext`: Request extensions containing the session (inserted by SessionMiddleware)
    ///
    /// # Returns
    /// The generated UUID that identifies this user session.
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

    /// Renews an existing session with a new expiration time.
    ///
    /// Used by the `/session/renew` endpoint when a user provides refreshed JWT credentials.
    /// This extends the session lifetime without requiring a full re-login.
    ///
    /// # Parameters
    /// - `exp`: New expiration time from the refreshed credentials
    pub fn renew(&self, exp: OffsetDateTime) -> Result<(), actix_session::SessionInsertError> {
        self.0.session.insert(EXP_KEY, exp.unix_timestamp()).and({
            self.0.session.renew();
            Ok(())
        })
    }

    /// Terminates the user session.
    ///
    /// Purges all data from the session cookie.
    pub fn logout(self) {
        self.0.session.purge();
    }

    /// Retrieves the user's unique identifier from the session.
    pub fn uid(&self) -> Result<Uuid, ExtractError> {
        self.0.get_identity()
    }

    /// Retrieves the session expiration time, if set.
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

/// Implements actix-web's extractor pattern for automatic authentication.
///
/// This trait integration allows `Identity` to be used as a function parameter in handlers.
/// When actix-web sees `Identity` in a handler signature, it automatically calls this
/// `from_request()` method to extract and validate the user's session.
///
/// ## How actix-web extractors work:
///
/// 1. Handler is called: `async fn endpoint(user: Identity) -> HttpResponse`
/// 2. actix-web sees `Identity` parameter and calls `Identity::from_request()`
/// 3. This method extracts `IdentityInner` from request extensions (placed by middleware)
/// 4. If extraction succeeds → handler executes with the `Identity` value
/// 5. If extraction fails → HTTP 401 response is returned, handler never executes
///
/// This pattern eliminates manual authentication checks in every handler.
impl FromRequest for Identity {
    type Error = Error;
    type Future = Ready<Result<Self, Self::Error>>;

    #[inline]
    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        ready(Identity::extract(&req.extensions()).map_err(|err| {
            // Convert extraction errors to HTTP 401 Unauthorized
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
