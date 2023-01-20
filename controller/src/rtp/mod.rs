pub(crate) mod requests;

use std::env;
use std::io;
use std::time::Duration;

use log::{info, warn};
use tokio::{fs, time};

use crate::error::{Error, Result};
use crate::{input_server, streamer_video, Controller};

// RTP seems to use multiple port for a session. Fix a large offset to get
// a safe port.
const AUDIO_PORT_OFFSET: u16 = 10;

pub async fn generate_video_sdp(request: requests::GenerateSdpRequest) -> Result<String> {
    // Run the streamer once to generate the SDP
    let sdp_path = env::temp_dir().join("video.sdp");
    let ret = fs::remove_file(sdp_path.clone()).await;
    if let Err(err) = ret {
        if err.kind() != io::ErrorKind::NotFound {
            warn!("Fail to delete SDP file");
            return Err(Error::IO(err));
        }
    }

    let mut streamer = streamer_video::Handler::new();
    streamer
        .start(
            "localhost",
            30000,
            request.yuv444,
            Some(
                sdp_path
                    .clone()
                    .into_os_string()
                    .into_string()
                    .or(Err(Error::Rejected))?,
            ),
        )
        .await?;

    // Wait for sdp file to be available
    let mut sdp_content = None;

    for _ in 0..50 {
        let ret = fs::read_to_string(&sdp_path).await;
        match ret {
            Ok(file_content) => {
                info!("Got SDP file");
                sdp_content = Some(file_content);
                break;
            }
            Err(_) => {
                info!("Waiting for SDP file");
                time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
    }

    let ret = streamer.stop().await;
    info!("Streamer stop result: {:?}", ret);

    // Parse SDP content
    for l in sdp_content.ok_or(Error::Rejected)?.lines() {
        if l.starts_with("a=fmtp") {
            return Ok(l.to_string());
        }
    }

    Err(Error::Rejected)
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

    // Stop input_server if any
    if let Some(input_server) = ctrl.input_server.as_mut() {
        info!("Stop running Input Server");

        if input_server.is_running() {
            input_server.stop().await?;
        }
        ctrl.input_server = None
    }

    // SDP file will contain some required parameters on client side.
    info!("Start video. ip: {} - port: {}", request.ip, request.port);
    let mut streamer_video = streamer_video::Handler::new();
    streamer_video
        .start(request.ip.as_str(), request.port, request.yuv444, None)
        .await?;

    ctrl.streamer_video = Some(streamer_video);

    if request.audio {
        let audio_port = request.port + AUDIO_PORT_OFFSET;
        info!("Start audio. ip: {} - port: {audio_port}", request.ip);

        let mut streamer_audio = crate::streamer_audio::Handler::new();
        streamer_audio
            .start(request.ip.as_str(), audio_port)
            .await?;

        ctrl.streamer_audio = Some(streamer_audio);
    }

    if request.inputs {
        let mut input_server = input_server::Handler::new();
        input_server.start().await?;

        ctrl.input_server = Some(input_server);
    } else {
        warn!("Input server disabled");
    }

    time::sleep(Duration::from_millis(100)).await;
    Ok(())
}
