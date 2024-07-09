pub(crate) mod requests;
pub(crate) mod router;

use std::collections::HashSet;
use std::sync::Arc;

use actix_web::{web, HttpRequest, HttpResponse};
use tokio::sync::Mutex;

use crate::utils::TypeConvert;
use crate::{avservice, inputservice};
use crate::{Controller, DataPlaneBackend};
use crate::{Error, Result};

pub(crate) use router::WsRouter;

const VIDEO_STREAM_IDX: u16 = 0;
const AUDIO_STREAM_IDX: u16 = 1;
const INPUT_STREAM_IDX: u16 = 2;
const WS_LOCAL_CLIENTS_PORT: u16 = 9090;

pub(crate) struct WebsocketBackend {
    pub(crate) router: Arc<WsRouter>,
}

impl WebsocketBackend {
    pub(crate) fn new(router: Arc<WsRouter>) -> Self {
        Self { router }
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.router.stop().await?;

        Ok(())
    }
}

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
    if ctrl.has_data_plane()? {
        return Err(Error::DataPlaneAlreadyOpened);
    }

    // Start services
    let mut pending_services = HashSet::new();

    // Start WsRouter
    let ws_router = WsRouter::new();
    let service_listener = ws_router.listen_for_clients().await?;

    // Start VideoServer
    let avservice_video = if let Some(video_request) = request.video.as_ref() {
        let uri = format!("kymux://127.0.0.1:{WS_LOCAL_CLIENTS_PORT}/{VIDEO_STREAM_IDX:X}");

        let mut avservice_video =
            avservice::Handler::new(avservice::HandlerType::Video, ctrl.state_machine_proxy()?);
        avservice_video
            .kymux_start_video(
                &ctrl.process_factory,
                video_request.display_id,
                uri,
                request.bitrate,
                video_request.codec.clone(),
                video_request.yuv444,
                video_request.intra_refresh,
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

        let mut avservice_audio =
            avservice::Handler::new(avservice::HandlerType::Audio, ctrl.state_machine_proxy()?);
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
            gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
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
    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let backend = WebsocketBackend::new(ws_router);
    client_session.data_plane = Some(DataPlaneBackend::Websocket(backend));

    let avservice_video = if let Some(avservice_video) = avservice_video {
        vec![avservice_video]
    } else {
        vec![]
    };

    ctrl.services.video = avservice_video;
    ctrl.services.audio = avservice_audio;
    ctrl.services.input = inputservice;

    Ok(())
}

async fn start_video(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse> {
    let (res, session, msg_stream) = actix_ws::handle(&request, stream)?;
    let mut ctrl = ctrl.lock().await;

    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let Some(DataPlaneBackend::Websocket(backend)) = client_session.data_plane.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    backend
        .router
        .start_video_routing(session, msg_stream)
        .await?;

    Ok(res)
}

async fn start_audio(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse> {
    let (res, session, msg_stream) = actix_ws::handle(&request, stream)?;
    let mut ctrl = ctrl.lock().await;

    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let Some(DataPlaneBackend::Websocket(backend)) = client_session.data_plane.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    backend
        .router
        .start_audio_routing(session, msg_stream)
        .await?;

    Ok(res)
}

async fn start_inputs(
    ctrl: web::Data<Mutex<Controller>>,
    request: HttpRequest,
    stream: web::Payload,
) -> Result<HttpResponse> {
    let (res, session, msg_stream) = actix_ws::handle(&request, stream)?;
    let mut ctrl = ctrl.lock().await;

    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let Some(DataPlaneBackend::Websocket(backend)) = client_session.data_plane.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    backend
        .router
        .start_input_routing(session, msg_stream)
        .await?;

    Ok(res)
}
