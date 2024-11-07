use actix_web::{get, post, web, HttpRequest, HttpResponse};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use inputservice_types::CursorFormat;

use crate::utils::generate_session_id;
use crate::{controller::Controller, session::identity::Identity};

#[derive(Debug, Serialize)]
pub struct HelloResponse {
    pub ip: String,
}

#[derive(Debug, Deserialize)]
pub struct GenerateSdpRequest {
    pub yuv444: bool,
    pub display_id: u32,
    pub intra_refresh: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct GenerateSdpResponse {
    pub params: String,
}

#[derive(Debug, Deserialize)]
pub struct StartRequestVideo {
    pub port: u16,
    pub display_id: u32,
    pub yuv444: bool,
    pub intra_refresh: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct StartRequestAudio {
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
    pub(crate) gamepad_force_xbox360: Option<bool>,
    #[serde(default)]
    pub(crate) clipboard: bool,
}

#[derive(Debug, Deserialize)]
pub struct StartRequest {
    pub ip: String,
    pub bitrate: u32,
    pub video: Option<StartRequestVideo>,
    pub audio: Option<StartRequestAudio>,
    pub(crate) inputs: Option<StartRequestInput>,
}

#[derive(Debug, Serialize)]
pub struct StartResponse {
    pub session_id: Option<u32>,
}

#[get("/hello")]
async fn hello(req: HttpRequest, _user: Identity) -> HttpResponse {
    let peer_addr = if let Some(peer_addr) = req.peer_addr() {
        peer_addr
    } else {
        return HttpResponse::Forbidden().finish();
    };

    HttpResponse::Ok().json(HelloResponse {
        ip: peer_addr.ip().to_string(),
    })
}

#[post("/generate_sdp")]
async fn generate_sdp(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<GenerateSdpRequest>,
    _user: Identity,
) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    // Check request validity
    if !ctrl.display_known(request.display_id) {
        warn!("Unknown Display Id {}", request.display_id);
        return HttpResponse::Forbidden().finish();
    }

    let ret = super::generate_video_sdp(&mut ctrl, request.into_inner()).await;
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
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
    _user: Identity,
) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    // Check request validity
    if let Some(video_request) = request.video.as_ref() {
        if !ctrl.display_known(video_request.display_id) {
            warn!("Unknown Display Id {}", video_request.display_id);
            return HttpResponse::Forbidden().finish();
        }
    }

    let ret = super::start(&mut ctrl, request.into_inner()).await;
    match ret {
        Ok(_) => {
            let session_id = generate_session_id();
            info!("Stream started. SessionId: {session_id}");

            HttpResponse::Ok().json(StartResponse {
                session_id: Some(session_id),
            })
        }
        Err(err) => {
            info!("Stream start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}
