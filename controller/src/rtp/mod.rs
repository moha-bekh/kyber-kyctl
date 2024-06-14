pub(crate) mod requests;

use std::time::Duration;

use log::{info, warn};
use tokio::time;

use crate::error::{Error, Result};
use crate::utils::TypeConvert;
use crate::{avservice, inputservice, Controller, DataPlaneBackend};

pub async fn generate_video_sdp(
    ctrl: &mut Controller,
    request: requests::GenerateSdpRequest,
) -> Result<String> {
    let state_machine_proxy = ctrl.state_machine_proxy()?;

    let avservice = avservice::Handler::new(avservice::HandlerType::VideoSdp, state_machine_proxy);
    avservice
        .rtp_generate_video_sdp(
            &ctrl.process_factory,
            request.display_id,
            request.yuv444,
            request.intra_refresh,
        )
        .await
}

pub async fn start(ctrl: &mut Controller, request: requests::StartRequest) -> Result<()> {
    // SDP file will contain some required parameters on client side.
    if let Some(video_request) = request.video {
        info!(
            "Start video. ip: {} - port: {}",
            request.ip, video_request.port
        );

        let mut avservice =
            avservice::Handler::new(avservice::HandlerType::Video, ctrl.state_machine_proxy()?);
        avservice
            .rtp_start_video(
                &ctrl.process_factory,
                video_request.display_id,
                request.ip.clone(),
                video_request.port,
                request.bitrate,
                video_request.yuv444,
                video_request.intra_refresh,
            )
            .await?;

        ctrl.services.video = vec![avservice];
    }

    if let Some(config) = request.audio {
        info!("Start audio. ip: {} - port: {}", request.ip, config.port);

        let mut avservice_audio =
            avservice::Handler::new(avservice::HandlerType::Audio, ctrl.state_machine_proxy()?);
        avservice_audio
            .rtp_start_audio(&ctrl.process_factory, request.ip, config.port)
            .await?;

        ctrl.services.audio = Some(avservice_audio);
    }

    if let Some(inputs_request) = request.inputs {
        let mut inputservice = inputservice::Handler::new();

        let host_config = inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        let service_config = inputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format,
        };

        inputservice
            .start_tcp(&ctrl.process_factory, host_config, service_config)
            .await?;

        ctrl.services.input = Some(inputservice);
    } else {
        warn!("Input server disabled");
    }

    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    client_session.data_plane = Some(DataPlaneBackend::Rtp);

    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
