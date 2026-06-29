// Project Kyber: webclient.rs
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
    path::{Path, PathBuf},
    pin::Pin,
};

use actix_web::dev::{Service, ServiceRequest, ServiceResponse, Transform, forward_ready};
use actix_web::http::header::{self, HeaderValue};
use log::warn;

use crate::WEBCLIENT_ROOT;

pub struct WebClient;

impl<S, B> Transform<S, ServiceRequest> for WebClient
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = actix_web::Error;
    type InitError = ();
    type Transform = WebClientMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(WebClientMiddleware { service }))
    }
}

pub struct WebClientMiddleware<S> {
    service: S,
}

type LocalBoxFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

impl<S, B> Service<ServiceRequest> for WebClientMiddleware<S>
where
    S: Service<ServiceRequest, Response = ServiceResponse<B>, Error = actix_web::Error>,
    S::Future: 'static,
    B: 'static,
{
    type Response = ServiceResponse<B>;
    type Error = actix_web::Error;
    type Future = LocalBoxFuture<Result<Self::Response, Self::Error>>;

    forward_ready!(service);

    fn call(&self, req: ServiceRequest) -> Self::Future {
        let fut = self.service.call(req);

        Box::pin(async move {
            let mut res = fut.await?;
            let headers = res.headers_mut();

            headers.insert(
                header::CROSS_ORIGIN_OPENER_POLICY,
                HeaderValue::from_static("same-origin"),
            );
            headers.insert(
                header::CROSS_ORIGIN_EMBEDDER_POLICY,
                HeaderValue::from_static("require-corp"),
            );

            Ok(res)
        })
    }
}

pub fn root(override_path: Option<&Path>) -> Option<String> {
    let webclient_root: PathBuf = if let Some(p) = override_path {
        p.to_path_buf()
    } else {
        let Ok(mut path) = std::env::current_exe() else {
            warn!("Couldn't find current exe path");
            return None;
        };

        let mut webclient_root = if cfg!(target_os = "windows") {
            path.pop();

            path
        } else if cfg!(target_os = "linux") {
            path.pop();
            path.pop();

            path
        } else {
            path
        };

        webclient_root.push(WEBCLIENT_ROOT);
        webclient_root
    };

    let attr = std::fs::metadata(webclient_root.clone());
    if let Ok(attr) = attr {
        if !attr.is_dir() {
            warn!("{webclient_root:?} isn't a directory");
            return None;
        }

        match webclient_root.into_os_string().into_string() {
            Ok(root) => Some(root),
            Err(err) => {
                warn!("Fail to convert webclient root to string: {err:?}");
                None
            }
        }
    } else {
        None
    }
}
