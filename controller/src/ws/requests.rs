use actix_web::{get, post, web, HttpRequest, HttpResponse};
use log::info;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use avservice_types::VideoCodec;
use inputservice_types::CursorFormat;

use crate::utils::generate_session_id;
use crate::Controller;

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
) -> HttpResponse {
    let ret = super::start_handler(ctrl, request).await;
    match ret {
        Ok(_) => {
            let session_id = generate_session_id();

            info!("Websocket backend started. SessionId: {session_id}");
            HttpResponse::Ok().json(StartResponse {
                session_id: Some(session_id),
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
) -> HttpResponse {
    let ret = super::start_video(ctrl, request, stream).await;
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
) -> HttpResponse {
    let ret = super::start_audio(ctrl, request, stream).await;
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
) -> HttpResponse {
    let ret = super::start_inputs(ctrl, request, stream).await;
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
