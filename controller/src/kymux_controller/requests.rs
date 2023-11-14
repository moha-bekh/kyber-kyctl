use actix_web::{post, web, HttpResponse};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use avservice_types::VideoCodec;
use inputservice_types::CursorFormat;

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
        Ok(ret) => {
            info!("Kymux started. Listening on {port}", port = ret.port);
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
    pub(crate) video_protocol: Option<kymux::VideoProtocol>,
}

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
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    pub(crate) bitrate: u32,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) video: Option<StartRequestVideo>,
    pub(crate) audio: bool,
    pub(crate) inputs: Option<StartRequestInput>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct StartSessionResponse {
    pub(crate) video_endpoint: Option<u16>,
    pub(crate) audio_endpoint: Option<u16>,
    pub(crate) input_endpoint: Option<u16>,
}

#[post("/start_session")]
pub(crate) async fn start_session(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    // Check request validity
    if let Some(video_request) = request.video.as_ref() {
        if !ctrl.display_known(video_request.display_id) {
            warn!("Unknown Display Id {}", video_request.display_id);
            return HttpResponse::Forbidden().finish();
        }
    }

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

            HttpResponse::Ok().json(StartSessionResponse {
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
