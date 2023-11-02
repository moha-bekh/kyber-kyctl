use actix_web::{get, post, web, HttpRequest, HttpResponse};
use log::info;
use serde::Deserialize;
use tokio::sync::Mutex;

use avservice_types::VideoCodec;
use inputservice_types::CursorFormat;

use crate::Controller;

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequestVideo {
    pub(crate) display_id: u32,
    pub(crate) codec: VideoCodec,
    pub(crate) yuv444: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequestInput {
    pub(crate) cursor_format: CursorFormat,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequest {
    pub(crate) bitrate: u32,
    pub(crate) video: Option<StartRequestVideo>,
    pub(crate) audio: bool,
    pub(crate) inputs: Option<StartRequestInput>,
}

#[post("/start")]
async fn start_handler(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
) -> HttpResponse {
    let ret = super::start_handler(ctrl, request).await;
    match ret {
        Ok(_) => {
            info!("Websocket backend started");
            HttpResponse::Ok().finish()
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
