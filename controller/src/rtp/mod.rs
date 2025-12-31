pub(crate) mod requests;

use std::time::Duration;

use log::{info, warn};
use tokio::time;

use crate::error::{Error, Result};
use crate::utils::TypeConvert;
use crate::{avservice, inputservice, Controller, DataPlaneBackend};

pub async fn generate_video_sdp(
    ctrl: &mut Controller,
    session_id: uuid::Uuid,
    request: requests::GenerateSdpRequest,
) -> Result<String> {
    let state_machine_proxy = ctrl.state_machine_proxy()?;

    let avservice = avservice::Handler::new(
        avservice::HandlerType::VideoSdp,
        Some(session_id),
        state_machine_proxy,
    );
    avservice
        .rtp_generate_video_sdp(
            &ctrl.process_factory,
            request.display_id,
            request.yuv444,
            request.intra_refresh,
        )
        .await
}

pub async fn start(
    ctrl: &mut Controller,
    session_id: uuid::Uuid,
    request: requests::StartRequest,
) -> Result<()> {
    let state_machine_proxy = ctrl.state_machine_proxy()?;
    let process_factory = ctrl.process_factory.clone();
    let displays = ctrl.displays.convert();

    // Start services first (before accessing session)
    let mut avservice_video = None;
    let mut avservice_audio = None;
    let mut inputservice_handler = None;

    // SDP file will contain some required parameters on client side.
    if let Some(video_request) = request.video {
        info!(
            "Start video. ip: {} - port: {}",
            request.ip, video_request.port
        );

        let mut avservice = avservice::Handler::new(
            avservice::HandlerType::Video,
            Some(session_id),
            state_machine_proxy.clone(),
        );
        avservice
            .rtp_start_video(
                &process_factory,
                video_request.display_id,
                request.ip.clone(),
                video_request.port,
                request.bitrate,
                video_request.yuv444,
                video_request.intra_refresh,
            )
            .await?;

        avservice_video = Some(avservice);
    }

    if let Some(config) = request.audio {
        info!("Start audio. ip: {} - port: {}", request.ip, config.port);

        let mut avservice = avservice::Handler::new(
            avservice::HandlerType::Audio,
            Some(session_id),
            state_machine_proxy.clone(),
        );
        avservice
            .rtp_start_audio(&process_factory, request.ip, config.port)
            .await?;

        avservice_audio = Some(avservice);
    }

    if let Some(inputs_request) = request.inputs {
        let mut inputservice = inputservice::Handler::new();

        let host_config = inputservice_types::HostConfig {
            display_list: displays,
        };

        let service_config = inputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format,
            gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
            clipboard: inputs_request.clipboard,
        };

        inputservice
            .start_tcp(&process_factory, host_config, service_config)
            .await?;

        inputservice_handler = Some(inputservice);
    } else {
        warn!("Input server disabled");
    }

    // Single session access to store all services
    let client_session = ctrl.get_session(&session_id)?;

    if let Some(avservice) = avservice_video {
        client_session.services.video = vec![avservice];
    }
    client_session.services.audio = avservice_audio;
    client_session.services.input = inputservice_handler;

    let Some(streaming_session) = client_session.stream_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    streaming_session.data_plane = Some(DataPlaneBackend::Rtp);

    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
