pub(crate) mod requests;
pub(crate) mod router;

use std::collections::HashSet;

use actix_web::{web, HttpRequest, HttpResponse};
use log::info;
use tokio::sync::Mutex;

use crate::controller::{self, Controller};
use crate::utils::TypeConvert;
use crate::{avservice, inputservice};
use crate::{Error, Result};

pub(crate) use router::WsRouter;

const VIDEO_STREAM_IDX: u16 = 0;
const AUDIO_STREAM_IDX: u16 = 1;
const INPUT_STREAM_IDX: u16 = 2;
const WS_LOCAL_CLIENTS_PORT: u16 = 9090;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Service {
    Video,
    Audio,
    Input,
}

async fn start_handler(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<requests::StartRequest>,
) -> Result<()> {
    let mut ctrl = ctrl.lock().await;

    // Stop running components
    let ret = controller::stop_avservice(ctrl.avservice_video.take()).await;
    info!("Video AvServer stopped. ret={ret:?}");

    let ret = controller::stop_avservice(ctrl.avservice_audio.take()).await;
    info!("Audio AvServer stopped. ret={ret:?}");

    let ret = controller::stop_input(ctrl.inputservice.take()).await;
    info!("InputServer stopped. ret={ret:?}");

    let ret = controller::stop_kymux(ctrl.kymux_server.take(), ctrl.kymux_connection.take()).await;
    info!("Kymux stopped. ret={ret:?}");

    let ret = controller::stop_wsrouter(ctrl.ws_router.take()).await;
    info!("WsRouter stopped. ret={ret:?}");

    // Start services
    let mut pending_services = HashSet::new();

    // Start WsRouter
    let ws_router = WsRouter::new();
    let service_listener = ws_router.listen_for_clients().await?;

    // Start VideoServer
    let avservice_video = if let Some(video_request) = request.video.as_ref() {
        let uri = format!("kymux://127.0.0.1:{WS_LOCAL_CLIENTS_PORT}/{VIDEO_STREAM_IDX:X}");

        let mut avservice_video = avservice::Handler::new("Video");
        avservice_video
            .kymux_start_video(
                &ctrl.process_factory,
                video_request.display_id,
                uri,
                request.bitrate,
                video_request.codec.clone(),
                video_request.yuv444,
            )
            .await?;

        pending_services.insert(Service::Video);

        Some(avservice_video)
    } else {
        None
    };

    // Start AudioServer
    let avservice_audio = if request.audio {
        let uri = format!("kymux://127.0.0.1:{WS_LOCAL_CLIENTS_PORT}/{AUDIO_STREAM_IDX:X}");

        let mut avservice_audio = avservice::Handler::new("Audio");
        avservice_audio
            .kymux_start_audio(&ctrl.process_factory, uri)
            .await?;

        pending_services.insert(Service::Audio);

        Some(avservice_audio)
    } else {
        None
    };

    // Start InputServer
    let inputservice = if let Some(inputs_request) = &request.inputs {
        let host_config = inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        let service_config = inputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format.clone(),
        };

        let uri = format!("kymux://127.0.0.1:{WS_LOCAL_CLIENTS_PORT}/{INPUT_STREAM_IDX:X}");

        let mut inputservice = inputservice::Handler::new();
        inputservice
            .start_kymux(&ctrl.process_factory, host_config, service_config, uri)
            .await?;

        pending_services.insert(Service::Input);

        Some(inputservice)
    } else {
        None
    };

    // Wait for all services to be ready
    service_listener.wait_all(pending_services).await?;

    // Store objects
    ctrl.ws_router = Some(ws_router);
    ctrl.avservice_video = avservice_video;
    ctrl.avservice_audio = avservice_audio;
    ctrl.inputservice = inputservice;

    Ok(())
}

async fn start_video(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse> {
    let (res, session, msg_stream) = actix_ws::handle(&request, stream)?;
    let mut ctrl = ctrl.lock().await;

    let Some(ws_router) = ctrl.ws_router.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    ws_router.start_video_routing(session, msg_stream).await?;

    Ok(res)
}

async fn start_audio(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse> {
    let (res, session, msg_stream) = actix_ws::handle(&request, stream)?;
    let mut ctrl = ctrl.lock().await;

    let Some(ws_router) = ctrl.ws_router.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    ws_router.start_audio_routing(session, msg_stream).await?;

    Ok(res)
}

async fn start_inputs(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse> {
    let (res, session, msg_stream) = actix_ws::handle(&request, stream)?;
    let mut ctrl = ctrl.lock().await;

    let Some(ws_router) = ctrl.ws_router.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    ws_router.start_input_routing(session, msg_stream).await?;

    Ok(res)
}
