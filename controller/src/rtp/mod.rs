pub(crate) mod requests;

use std::time::Duration;

use log::{info, warn};
use tokio::time;

use crate::error::Result;
use crate::{avservice, inputservice, Controller};

// RTP seems to use multiple port for a session. Fix a large offset to get
// a safe port.
const AUDIO_PORT_OFFSET: u16 = 10;

pub async fn generate_video_sdp(
    ctrl: &mut Controller,
    request: requests::GenerateSdpRequest,
) -> Result<String> {
    let avservice = avservice::Handler::new("VideoSDP");
    avservice
        .rtp_generate_video_sdp(&mut ctrl.process_factory, request.yuv444)
        .await
}

pub async fn start(ctrl: &mut Controller, request: requests::StartRequest) -> Result<()> {
    // Stop avservice if any
    if let Some(avservice) = ctrl.avservice_video.as_mut() {
        info!("Stop running AV Video service");

        if avservice.is_running() {
            avservice.stop().await?;
        }
        ctrl.avservice_video = None
    }

    // Stop inputservice if any
    if let Some(inputservice) = ctrl.inputservice.as_mut() {
        info!("Stop running Input Server");

        if inputservice.is_running() {
            inputservice.stop().await?;
        }
        ctrl.inputservice = None
    }

    // SDP file will contain some required parameters on client side.
    info!("Start video. ip: {} - port: {}", request.ip, request.port);

    let mut avservice = avservice::Handler::new("Video");
    avservice
        .rtp_start_video(
            &mut ctrl.process_factory,
            request.ip.clone(),
            request.port,
            request.yuv444,
        )
        .await?;

    ctrl.avservice_video = Some(avservice);

    if request.audio {
        let audio_port = request.port + AUDIO_PORT_OFFSET;
        info!("Start audio. ip: {} - port: {audio_port}", &request.ip);

        let mut avservice_audio = avservice::Handler::new("Audio");
        avservice_audio
            .rtp_start_audio(&mut ctrl.process_factory, request.ip, audio_port)
            .await?;

        ctrl.avservice_audio = Some(avservice_audio);
    }

    if request.inputs {
        let mut inputservice = inputservice::Handler::new();
        inputservice.start_tcp(&mut ctrl.process_factory).await?;

        ctrl.inputservice = Some(inputservice);
    } else {
        warn!("Input server disabled");
    }

    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
