pub(crate) mod requests;

use std::time::Duration;

use log::{info, warn};
use tokio::time;

use crate::error::Result;
use crate::{inputservice, streamer, Controller};

// RTP seems to use multiple port for a session. Fix a large offset to get
// a safe port.
const AUDIO_PORT_OFFSET: u16 = 10;

pub async fn generate_video_sdp(
    ctrl: &mut Controller,
    request: requests::GenerateSdpRequest,
) -> Result<String> {
    let streamer = streamer::Handler::new("VideoSDP");
    streamer
        .rtp_generate_video_sdp(&mut ctrl.process_factory, request.yuv444)
        .await
}

pub async fn start(ctrl: &mut Controller, request: requests::StartRequest) -> Result<()> {
    // Stop streamer if any
    if let Some(streamer) = ctrl.streamer_video.as_mut() {
        info!("Stop running streamer");

        if streamer.is_running() {
            streamer.stop().await?;
        }
        ctrl.streamer_video = None
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

    let mut streamer = streamer::Handler::new("Video");
    streamer
        .rtp_start_video(
            &mut ctrl.process_factory,
            request.ip.clone(),
            request.port,
            request.yuv444,
        )
        .await?;

    ctrl.streamer_video = Some(streamer);

    if request.audio {
        let audio_port = request.port + AUDIO_PORT_OFFSET;
        info!("Start audio. ip: {} - port: {audio_port}", &request.ip);

        let mut streamer_audio = streamer::Handler::new("Audio");
        streamer_audio
            .rtp_start_audio(&mut ctrl.process_factory, request.ip, audio_port)
            .await?;

        ctrl.streamer_audio = Some(streamer_audio);
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
