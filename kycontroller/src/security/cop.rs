// Project Kyber: cop.rs
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

use std::{
    future::{Future, Ready, ready},
    pin::Pin,
    rc::Rc,
};

use actix_web::{
    Error, HttpResponse,
    body::{BoxBody, MessageBody},
    dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready},
    http::{Method, header},
};

use super::TrustedOrigins;

/// Middleware that guards against Cross-Site Request Forgery by inspecting browser specific
/// metadata headers, following the algorithm described by Filippo Valsorda implemented in Go
/// 1.25's `net/http.CrossOriginProtection` middleware.
#[derive(Clone)]
pub struct CrossOriginProtection {
    origins: Rc<TrustedOrigins>,
}

impl CrossOriginProtection {
    pub(crate) fn new(origins: TrustedOrigins) -> Self {
        Self {
            origins: Rc::new(origins),
        }
    }
}

impl<S, B> Transform<S, ServiceRequest> for CrossOriginProtection
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Transform = CrossOriginProtectionMiddleware<S>;
    type InitError = ();
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(CrossOriginProtectionMiddleware {
            service,
            origins: self.origins.clone(),
        }))
    }
}

pub struct CrossOriginProtectionMiddleware<S> {
    service: S,
    origins: Rc<TrustedOrigins>,
}

impl<S, B> Service<ServiceRequest> for CrossOriginProtectionMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = Error> + 'static,
    B: MessageBody + 'static,
{
    type Response = ServiceResponse<BoxBody>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        // Safe methods are always allowed.
        if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
            let next = self.service.call(req);
            return Box::pin(async move { next.await.map(|r| r.map_into_boxed_body()) });
        }

        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok());

        let sfs = req
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok());

        if is_safe(&self.origins, origin, sfs) {
            let fut = self.service.call(req);
            Box::pin(async move { fut.await.map(|r| r.map_into_boxed_body()) })
        } else {
            let resp = req.into_response(
                HttpResponse::Forbidden().body("Cross origin protection checks failed"),
            );
            Box::pin(ready(Ok(resp)))
        }
    }
}

fn is_safe(trusted_origins: &TrustedOrigins, origin: Option<&str>, sfs: Option<&str>) -> bool {
    // Allow origins matching the trusted list.
    if let Some(o) = origin
        && trusted_origins.contains(o)
    {
        return true;
    }

    // Allow same-origin content reported by Sec-Fetch-Site.
    if let Some(sfs) = sfs {
        return matches!(sfs, "same-origin" | "none");
    }

    if origin.is_none() {
        // Neither Origin nor Sec-Fetch-Site is typical of non-browser clients.
        // Cross origin abuses are not a threat.
        return true;
    };

    // Untrusted origin present without Sec-Fetch-Site, the request is from a buggy or pre-2020
    // browser Usually, the last resort check is based on the Host header, but, as Kyber does not
    // plan on supporting browsers that not supporting the recent Webcodec API, the request is just
    // dropped.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::TrustedOrigins;
    use actix_web::{
        App, HttpResponse,
        http::{Method, StatusCode},
        test, web,
    };

    fn no_origins() -> TrustedOrigins {
        TrustedOrigins::Some(vec![])
    }

    fn trusted_origin(origin: &str) -> TrustedOrigins {
        TrustedOrigins::Some(vec![origin.to_owned()])
    }

    async fn ok_handler() -> HttpResponse {
        HttpResponse::Ok().finish()
    }

    #[actix_web::test]
    async fn allows_safe_methods() {
        let app = test::init_service(
            App::new()
                .wrap(CrossOriginProtection::new(no_origins()))
                .route("/test", web::route().to(ok_handler)),
        )
        .await;

        for method in [Method::GET, Method::HEAD, Method::OPTIONS] {
            let req = test::TestRequest::default()
                .method(method.clone())
                .uri("/test")
                .insert_header(("sec-fetch-site", "cross-site"))
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "{method} should bypass cross-origin protection",
            );
        }
    }

    #[actix_web::test]
    async fn sec_fetch_site_values() {
        let cases: &[(&str, StatusCode)] = &[
            ("same-origin", StatusCode::OK),
            ("none", StatusCode::OK),
            ("cross-site", StatusCode::FORBIDDEN),
            ("same-site", StatusCode::FORBIDDEN),
        ];

        let app = test::init_service(
            App::new()
                .wrap(CrossOriginProtection::new(no_origins()))
                .route("/test", web::post().to(ok_handler)),
        )
        .await;

        for &(sfs, want) in cases {
            let req = test::TestRequest::post()
                .uri("/test")
                .insert_header(("sec-fetch-site", sfs))
                .to_request();
            let resp = test::call_service(&app, req).await;
            assert_eq!(resp.status(), want, "Sec-Fetch-Site: {sfs}");
        }
    }

    #[actix_web::test]
    async fn allows_non_browser_clients() {
        let app = test::init_service(
            App::new()
                .wrap(CrossOriginProtection::new(no_origins()))
                .route("/test", web::post().to(ok_handler)),
        )
        .await;

        let req = test::TestRequest::post().uri("/test").to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn allows_trusted_origin() {
        let app = test::init_service(
            App::new()
                .wrap(CrossOriginProtection::new(trusted_origin(
                    "https://trusted.example",
                )))
                .route("/test", web::post().to(ok_handler)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/test")
            .insert_header(("origin", "https://trusted.example"))
            .insert_header(("sec-fetch-site", "cross-site"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[actix_web::test]
    async fn forbids_untrusted_origin() {
        let app = test::init_service(
            App::new()
                .wrap(CrossOriginProtection::new(trusted_origin(
                    "https://trusted.example",
                )))
                .route("/test", web::post().to(ok_handler)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/test")
            .insert_header(("origin", "https://evil.example"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[actix_web::test]
    async fn allows_any_cross_origin_request() {
        let app = test::init_service(
            App::new()
                .wrap(CrossOriginProtection::new(TrustedOrigins::All))
                .route("/test", web::post().to(ok_handler)),
        )
        .await;

        let req = test::TestRequest::post()
            .uri("/test")
            .insert_header(("origin", "https://any.example"))
            .insert_header(("sec-fetch-site", "cross-site"))
            .to_request();
        let resp = test::call_service(&app, req).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
