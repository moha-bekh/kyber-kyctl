pub(crate) mod requests;

use std::time::Duration;

use log::{info, warn};
use tokio::time;

use crate::error::Result;
use crate::utils::TypeConvert;
use crate::{avservice, inputservice, Controller};

pub async fn generate_video_sdp(
    ctrl: &mut Controller,
    request: requests::GenerateSdpRequest,
) -> Result<String> {
    let avservice = avservice::Handler::new("VideoSDP");
    avservice
        .rtp_generate_video_sdp(&ctrl.process_factory, request.display_id, request.yuv444)
        .await
}

pub async fn start(ctrl: &mut Controller, request: requests::StartRequest) -> Result<()> {
    // Stop video avservice if any
    if let Some(mut avservice) = ctrl.avservice_video.take() {
        info!("Stop running AV Video service");

        if avservice.is_running() {
            avservice.stop().await?;
        }
    }

    // Stop audio avservice if any
    if let Some(mut avservice) = ctrl.avservice_audio.take() {
        info!("Stop running AV Audio service");

        if avservice.is_running() {
            avservice.stop().await?;
        }
    }

    // Stop inputservice if any
    if let Some(mut inputservice) = ctrl.inputservice.take() {
        info!("Stop running Input Server");

        if inputservice.is_running() {
            inputservice.stop().await?;
        }
    }

    // SDP file will contain some required parameters on client side.
    if let Some(video_request) = request.video {
        info!(
            "Start video. ip: {} - port: {}",
            request.ip, video_request.port
        );

        let mut avservice = avservice::Handler::new("Video");
        avservice
            .rtp_start_video(
                &ctrl.process_factory,
                video_request.display_id,
                request.ip.clone(),
                video_request.port,
                video_request.yuv444,
            )
            .await?;

        ctrl.avservice_video = Some(avservice);
    }

    if let Some(config) = request.audio {
        info!("Start audio. ip: {} - port: {}", request.ip, config.port);

        let mut avservice_audio = avservice::Handler::new("Audio");
        avservice_audio
            .rtp_start_audio(&ctrl.process_factory, request.ip, config.port)
            .await?;

        ctrl.avservice_audio = Some(avservice_audio);
    }

    if request.inputs {
        let mut inputservice = inputservice::Handler::new();

        let host_config = inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        inputservice
            .start_tcp(&ctrl.process_factory, host_config)
            .await?;

        ctrl.inputservice = Some(inputservice);
    } else {
        warn!("Input server disabled");
    }

    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
