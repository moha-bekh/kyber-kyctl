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

use actix_web::{post, web, HttpResponse};
use kymux::kyproto;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use avservice_types::VideoCodec;
use kynputservice_types::CursorFormat;

use crate::utils::generate_session_id;
use crate::{controller::Controller, session::identity::Identity};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartKymux {
    pub(crate) public_streamer_port: u16,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct StartKymuxCertificateHash {
    pub(crate) algorithm: String,
    pub(crate) hash: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct StartKymuxResponse {
    pub(crate) port: u16,
    pub(crate) certificate_hash: Option<StartKymuxCertificateHash>,
    pub(crate) token: String,
}

#[post("/start_mux")]
pub(crate) async fn start_mux(
    user: Identity,
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartKymux>,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            warn!("Failed to extract session UID from Identity");
            return HttpResponse::Unauthorized().finish();
        }
    };

    // Assume that the public port used for UDP is the same than the port for TCP.
    let ret = super::start_kymux(ctrl.clone(), session_id, request.public_streamer_port).await;
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
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    pub(crate) bitrate: u32,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) video: Option<Vec<StartRequestVideo>>,
    pub(crate) audio: bool,
    pub(crate) inputs: Option<StartRequestInput>,
    pub(crate) metrics: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionVideoResponse {
    pub(crate) display_id: u32,
    pub(crate) endpoint: u16,
    pub(crate) metrics_endpoint: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionAudioResponse {
    pub(crate) endpoint: u16,
}
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionInputResponse {
    pub(crate) endpoint: u16,
    #[serde(default)]
    pub(crate) clipboard: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionClockSyncResponse {
    pub(crate) endpoint: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionMetricsResponse {
    pub(crate) endpoint: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionResponse {
    pub(crate) session_id: Option<u32>,
    pub(crate) video: Option<Vec<StartSessionVideoResponse>>,
    pub(crate) audio: Option<StartSessionAudioResponse>,
    pub(crate) input: Option<StartSessionInputResponse>,
    pub(crate) clock_sync: Option<StartSessionClockSyncResponse>,
    pub(crate) metrics: Option<StartSessionMetricsResponse>,
}

#[post("/start_session")]
pub(crate) async fn start_session(
    user: Identity,
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
) -> HttpResponse {
    let session_id = match user.uid() {
        Ok(uid) => uid,
        Err(_) => {
            warn!("Failed to extract session UID from Identity");
            return HttpResponse::Unauthorized().finish();
        }
    };

    let mut ctrl = ctrl.lock().await;

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
    let ret = super::start_session(&mut ctrl, session_id, request.into_inner()).await;
    match ret {
        Ok(response) => {
            let numeric_session_id = generate_session_id();
            info!("Session started. SessionId: {numeric_session_id}");

            let Ok(client_session) = ctrl.get_session(&session_id) else {
                error!(
                    "Session {} disappeared after successful start_session",
                    session_id
                );
                return HttpResponse::InternalServerError().finish();
            };

            let video = response.video.map(|video| {
                video
                    .into_iter()
                    .map(|video| {
                        client_session.services.video.push(video.avservice);

                        StartSessionVideoResponse {
                            display_id: video.display_id,
                            endpoint: video.endpoint,
                            metrics_endpoint: video.metrics_endpoint,
                        }
                    })
                    .collect()
            });

            let audio = if let Some(audio) = response.audio {
                client_session.services.audio = Some(audio.avservice);

                Some(StartSessionAudioResponse {
                    endpoint: audio.endpoint,
                })
            } else {
                None
            };

            let input = if let Some(input) = response.input {
                client_session.services.input = Some(input.inputservice);

                Some(StartSessionInputResponse {
                    endpoint: input.endpoint,
                    clipboard: input.clipboard,
                })
            } else {
                None
            };

            let clock_sync = if let Some(clock_sync) = response.clock_sync {
                client_session.services.clock_sync = Some(clock_sync.task);

                Some(StartSessionClockSyncResponse {
                    endpoint: clock_sync.endpoint,
                })
            } else {
                None
            };

            let metrics = if let Some(metrics) = response.metrics {
                client_session.services.metrics = Some(metrics.task);

                Some(StartSessionMetricsResponse {
                    endpoint: metrics.endpoint,
                })
            } else {
                None
            };

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
