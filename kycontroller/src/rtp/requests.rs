// Project Kyber: requests.rs
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

use std::net::IpAddr;

use actix_web::{HttpRequest, HttpResponse, get, post, web};
use log::{info, warn};

pub(crate) use kyctl_common::rtp::{GenerateSdpRequest, StartRequest};
use kyctl_common::rtp::{GenerateSdpResponse, HelloResponse, StartResponse};

use crate::utils::generate_session_id;
use crate::{controller::Controller, session::identity::Identity};

#[get("/hello")]
async fn hello(req: HttpRequest, _user: Identity) -> HttpResponse {
    let peer_addr = if let Some(peer_addr) = req.peer_addr() {
        peer_addr
    } else {
        return HttpResponse::Forbidden().finish();
    };

    // The HTTP listener is dual-stack (`[::]:8080`), so an IPv4 client is
    // reported as `::ffff:a.b.c.d`. Strict SDP parsers reject `c=IN IP4 ::ffff:...` and refuse the session.
    let ip = match peer_addr.ip() {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 @ IpAddr::V4(_) => v4,
    };

    HttpResponse::Ok().json(HelloResponse { ip: ip.to_string() })
}

#[post("/generate_sdp")]
async fn generate_sdp(
    ctrl: web::Data<Controller>,
    request: web::Json<GenerateSdpRequest>,
    user: Identity,
) -> HttpResponse {
    if user.uid().is_err() {
        return HttpResponse::Unauthorized().finish();
    };

    if ctrl.config.session_mode() == crate::config::SessionMode::Multi {
        warn!("RTP backend is disabled in multi-client mode");
        return HttpResponse::Conflict().json("RTP backend unavailable in multi-client mode");
    }

    // Check request validity
    if !ctrl.display_known(request.display_id) {
        warn!("Unknown Display Id {}", request.display_id);
        return HttpResponse::Forbidden().finish();
    }

    let ret = super::generate_video_sdp(&ctrl, request.into_inner()).await;
    match ret {
        Ok(params) => {
            info!("SDP generated: {}", params);

            HttpResponse::Ok().json(GenerateSdpResponse { params })
        }
        Err(err) => {
            info!("SDP generation rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[post("/start")]
async fn start(
    ctrl: web::Data<Controller>,
    request: web::Json<StartRequest>,
    user: Identity,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            return HttpResponse::Unauthorized().finish();
        }
    };

    if ctrl.config.session_mode() == crate::config::SessionMode::Multi {
        warn!("RTP backend is disabled in multi-client mode");
        return HttpResponse::Conflict().json("RTP backend unavailable in multi-client mode");
    }

    // Check request validity
    if let Some(video_request) = request.video.as_ref()
        && !ctrl.display_known(video_request.display_id)
    {
        warn!("Unknown Display Id {}", video_request.display_id);
        return HttpResponse::Forbidden().finish();
    }

    let ret = super::start(&ctrl, session_id, request.into_inner()).await;
    match ret {
        Ok(_) => {
            let numeric_session_id = generate_session_id();
            info!("Stream started. SessionId: {numeric_session_id}");

            HttpResponse::Ok().json(StartResponse {
                session_id: Some(numeric_session_id),
            })
        }
        Err(err) => {
            info!("Stream start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}
