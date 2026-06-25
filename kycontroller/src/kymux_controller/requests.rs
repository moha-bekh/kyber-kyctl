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

use actix_web::{HttpResponse, post, web};
use kymux::kyproto;
use log::{error, info, warn};
use serde::Deserialize;

pub(crate) use kyctl_common::kymux::{
    StartKymux, StartKymuxCertificateHash, StartKymuxResponse, StartRequestInput,
    StartRequestVideo, StartSessionAudioResponse, StartSessionClockSyncResponse,
    StartSessionInputResponse, StartSessionMetricsResponse, StartSessionResponse,
    StartSessionVideoResponse,
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

#[derive(Debug, Deserialize)]
pub(crate) struct KymuxConfig {
    pub(crate) video_protocol: Option<kyproto::VideoProtocol>,
    pub(crate) audio_protocol: Option<kyproto::AudioProtocol>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequest {
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    pub(crate) bitrate: u32,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) video: Option<Vec<StartRequestVideo>>,
    pub(crate) audio: bool,
    pub(crate) inputs: Option<StartRequestInput>,
    pub(crate) metrics: bool,
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
                    endpoint: clock_sync.endpoint,
                })
            } else {
                None
            };

            let metrics = if let Some(metrics) = response.metrics {
                services.metrics = Some(metrics.task);

                Some(StartSessionMetricsResponse {
                    endpoint: metrics.endpoint,
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
