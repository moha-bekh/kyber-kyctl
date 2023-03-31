use actix_web::{post, web, HttpResponse};
use log::info;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use avservice_types::VideoCodec;

use crate::controller::{self, Controller};

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartKymux {
    pub(crate) public_streamer_port: u16,
}

#[derive(Debug, Serialize)]
pub(crate) struct StartKymuxResponse {
    pub(crate) port: u16,
}

#[post("/start_mux")]
pub(crate) async fn start_mux(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartKymux>,
) -> HttpResponse {
    // Assume that the public port used for UDP is the same than the port for TCP.
    let ret = super::start_kymux(ctrl.clone(), request.public_streamer_port).await;
    match ret {
        Ok(port) => {
            info!("Kymux started. Listening on {port}");
            HttpResponse::Ok().json(StartKymuxResponse { port })
        }
        Err(err) => {
            info!("Kymux start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequestVideo {
    pub(crate) codec: VideoCodec,
    pub(crate) yuv444: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StartRequest {
    pub(crate) video: Option<StartRequestVideo>,
    pub(crate) audio: bool,
    pub(crate) inputs: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartRequestResponse {
    pub(crate) video_endpoint: Option<u64>,
    pub(crate) audio_endpoint: Option<u64>,
    pub(crate) input_endpoint: Option<u64>,
}

#[post("/start_session")]
pub(crate) async fn start_session(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    // Stop running components
    let ret = controller::stop_avservice(ctrl.avservice_video.take()).await;
    info!("AvServer stopped. ret={ret:?}");

    let ret = controller::stop_input(ctrl.inputservice.take()).await;
    info!("Inputserver stopped. ret={ret:?}");

    // Start session
    let ret = super::start_session(&mut ctrl, request.into_inner()).await;
    match ret {
        Ok(response) => {
            info!("Stream started");

            ctrl.avservice_video = response.avservice_video;
            ctrl.avservice_audio = response.avservice_audio;
            ctrl.inputservice = response.inputservice;

            HttpResponse::Ok().json(StartRequestResponse {
                video_endpoint: response.video_endpoint,
                audio_endpoint: response.audio_endpoint,
                input_endpoint: response.input_endpoint,
            })
        }
        Err(err) => {
            info!("Stream start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}
