// Project Kyber: requests.rs
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

use actix_web::{get, post, web, HttpRequest, HttpResponse};
use log::info;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use kyavservice_types::VideoCodec;
use kynputservice_types::CursorFormat;

use crate::utils::generate_session_id;
use crate::{session::identity::Identity, Controller};

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequestVideo {
    pub(crate) display_id: u32,
    pub(crate) codec: VideoCodec,
    pub(crate) yuv444: bool,
    pub(crate) intra_refresh: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
    pub(crate) gamepad_force_xbox360: Option<bool>,
    #[serde(default)]
    pub(crate) clipboard: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequest {
    pub(crate) bitrate: u32,
    pub(crate) video: Option<StartRequestVideo>,
    pub(crate) audio: bool,
    pub(crate) inputs: Option<StartRequestInput>,
}

#[derive(Debug, Serialize)]
pub struct StartResponse {
    pub session_id: Option<u32>,
}

#[post("/start")]
async fn start_handler(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
    user: Identity,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            return HttpResponse::Unauthorized().finish();
        }
    };

    let ret = super::start_handler(ctrl, session_id, request).await;
    match ret {
        Ok(_) => {
            let numeric_session_id = generate_session_id();

            info!("Websocket backend started. SessionId: {numeric_session_id}");
            HttpResponse::Ok().json(StartResponse {
                session_id: Some(numeric_session_id),
            })
        }
        Err(err) => {
            info!("Websocket start backend rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[get("/video")]
async fn video(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            return HttpResponse::Unauthorized().finish();
        }
    };

    let ret = super::start_video(ctrl, session_id, request, stream).await;
    match ret {
        Ok(response) => {
            info!("Websocket video started");
            response
        }
        Err(err) => {
            info!("Websocket video start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[get("/audio")]
async fn audio(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            return HttpResponse::Unauthorized().finish();
        }
    };

    let ret = super::start_audio(ctrl, session_id, request, stream).await;
    match ret {
        Ok(response) => {
            info!("Websocket audio started");
            response
        }
        Err(err) => {
            info!("Websocket audio start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[get("/inputs")]
async fn inputs(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            return HttpResponse::Unauthorized().finish();
        }
    };

    let ret = super::start_inputs(ctrl, session_id, request, stream).await;
    match ret {
        Ok(response) => {
            info!("Websocket inputs started");
            response
        }
        Err(err) => {
            info!("Websocket inputs start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}
