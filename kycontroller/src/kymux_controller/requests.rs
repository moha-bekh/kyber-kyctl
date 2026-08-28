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

use actix_web::{HttpRequest, HttpResponse, get, post, web};
use log::{error, info, warn};

pub(crate) use kyctl_common::kymux::{
    StartKymux, StartKymuxCertificateHash, StartKymuxResponse, StartRequest,
    StartSessionAudioResponse, StartSessionClockSyncResponse, StartSessionInputResponse,
    StartSessionMetricsResponse, StartSessionResponse, StartSessionVideoResponse,
};

use crate::controller::StreamingServices;
use crate::utils::generate_session_id;
use crate::{controller::Controller, session::identity::Identity};

#[post("/start_mux")]
pub(crate) async fn start_mux(
    user: Identity,
    ctrl: web::Data<Controller>,
    data: web::Json<StartKymux>,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            warn!("Failed to extract session UID from Identity");
            return HttpResponse::Unauthorized().finish();
        }
    };

    // Assume that the public port used for UDP is the same than the port for TCP.
    let ret = super::start_kymux(ctrl.clone(), session_id, data.public_streamer_port).await;
    match ret {
        Ok(ret) => {
            info!("Kymux started. Public port: {port}", port = ret.port);
            HttpResponse::Ok().json(ret)
        }
        Err(err) => {
            info!("Kymux start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[post("/start_session")]
pub(crate) async fn start_session(
    user: Identity,
    ctrl: web::Data<Controller>,
    request: web::Json<StartRequest>,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            warn!("Failed to extract session UID from Identity");
            return HttpResponse::Unauthorized().finish();
        }
    };

    // Check request validity
    if let Some(video_request) = request.video.as_ref() {
        for request in video_request {
            if !ctrl.display_known(request.display_id) {
                warn!("Unknown Display Id {}", request.display_id);
                return HttpResponse::Forbidden().finish();
            }
        }
    }

    // Start session
    let ret = super::start_session(&ctrl, session_id, request.into_inner()).await;
    match ret {
        Ok(response) => {
            let numeric_session_id = generate_session_id();
            info!("Session started. SessionId: {numeric_session_id}");

            let mut services = StreamingServices::default();

            let video = response.video.map(|video| {
                video
                    .into_iter()
                    .map(|v| {
                        if let Some(output) = v.output {
                            services.tasks.push(output);
                        }
                        if let Some(m) = v.metrics_output {
                            services.tasks.push(m);
                        }
                        services.video_subscriptions.push(v.subscription);

                        StartSessionVideoResponse {
                            display_id: v.display_id,
                            endpoint: v.endpoint,
                            metrics_endpoint: v.metrics_endpoint,
                        }
                    })
                    .collect()
            });

            let audio = if let Some(audio) = response.audio {
                if let Some(output) = audio.output {
                    services.tasks.push(output);
                }
                services.audio_subscription = Some(audio.subscription);

                Some(StartSessionAudioResponse {
                    endpoint: audio.endpoint,
                })
            } else {
                None
            };

            let input = if let Some(input) = response.input {
                services.input = Some(input.inputservice);

                Some(StartSessionInputResponse {
                    endpoint: input.endpoint,
                    clipboard: input.clipboard,
                })
            } else {
                None
            };

            let clock_sync = if let Some(clock_sync) = response.clock_sync {
                services.clock_sync = Some(clock_sync.task);

                Some(StartSessionClockSyncResponse {
                    endpoint: Some(clock_sync.endpoint),
                })
            } else {
                None
            };

            let metrics = if let Some(metrics) = response.metrics {
                services.metrics = Some(metrics.task);

                Some(StartSessionMetricsResponse {
                    endpoint: Some(metrics.endpoint),
                })
            } else {
                None
            };

            let res = ctrl.with_session(&session_id, |session| Ok(session.services.clone()));

            let Ok(session_services) = res else {
                error!("Session {session_id} disappeared after successful start_session",);
                return HttpResponse::InternalServerError().finish();
            };
            *session_services.lock().await = services;

            HttpResponse::Ok().json(StartSessionResponse {
                session_id: Some(numeric_session_id),
                video,
                audio,
                input,
                clock_sync,
                metrics,
            })
        }
        Err(err) => {
            info!("Stream start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

async fn ws_service_handler(
    ctrl: web::Data<Controller>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
    service_type: super::ServiceType,
    label: &str,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => return HttpResponse::Unauthorized().finish(),
    };

    let (res, session, msg_stream) = match actix_ws::handle(&request, stream) {
        Ok(v) => v,
        Err(err) => {
            info!("Websocket {label} upgrade failed: {err}");
            return HttpResponse::Forbidden().finish();
        }
    };

    match super::start_websocket_handler(ctrl, session_id, session, msg_stream, service_type).await
    {
        Ok(()) => {
            info!("Websocket {label} started");
            res
        }
        Err(err) => {
            info!("Websocket {label} start rejected: {err}");
            HttpResponse::Forbidden().finish()
        }
    }
}

#[get("/video")]
async fn ws_video(
    ctrl: web::Data<Controller>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
) -> HttpResponse {
    ws_service_handler(
        ctrl,
        request,
        stream,
        user,
        super::ServiceType::Video,
        "video",
    )
    .await
}

#[get("/audio")]
async fn ws_audio(
    ctrl: web::Data<Controller>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
) -> HttpResponse {
    ws_service_handler(
        ctrl,
        request,
        stream,
        user,
        super::ServiceType::Audio,
        "audio",
    )
    .await
}

#[get("/inputs")]
async fn ws_inputs(
    ctrl: web::Data<Controller>,
    request: HttpRequest,
    stream: web::Payload,
    user: Identity,
) -> HttpResponse {
    ws_service_handler(
        ctrl,
        request,
        stream,
        user,
        super::ServiceType::Input,
        "inputs",
    )
    .await
}
