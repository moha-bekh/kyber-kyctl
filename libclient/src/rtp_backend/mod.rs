pub mod host_service;
pub mod player;

use std::net::SocketAddr;

use libkynput_rs::Pipeline;

use crate::http::HttpClient;
use crate::{get_display_id, input_service};
use crate::{
    Display, InputSessionConfig, MessageProxy, Result, StartStreamResult, StreamingSessionConfig,
    VideoSessionConfig, WindowHandle,
};

pub use player::Player;

const RTP_VIDEO_PORT: u16 = 30000;

// RTP seems to use multiple port for a session. Fix a large offset to get
// a safe port.
const RTP_AUDIO_PORT: u16 = RTP_VIDEO_PORT + 10;

struct VideoConfig {
    display_id: u32,
    yuv444: bool,

    window_handle: WindowHandle,

    ip: String,
    port: u16,
    sdp_param: String,
}

struct AudioConfig {
    ip: String,
    port: u16,
}

async fn create_video_config(
    client: &HttpClient,
    displays: &Vec<Display>,
    ip: &str,
    config: &Option<VideoSessionConfig>,
) -> Result<Option<VideoConfig>> {
    let Some(config) = config.as_ref() else {
        return Ok(None);
    };

    let display_id = get_display_id(displays, config).id;

    let sdp_param = host_service::generate_sdp(client, config.yuv444, display_id).await?;

    Ok(Some(VideoConfig {
        display_id,
        yuv444: config.yuv444,
        ip: ip.into(),
        port: RTP_VIDEO_PORT,
        window_handle: config.window_handle,
        sdp_param,
    }))
}

fn start_input(
    streamer_ip: &str,
    config: &InputSessionConfig,
    msg_proxy: MessageProxy,
) -> Result<(Option<input_service::Client>, Option<Pipeline>)> {
    let create_stream_cb = |_target, msg_sink| libkynput_rs::tcp::new_client(streamer_ip, msg_sink);

    let input_service = input_service::Client::new(
        msg_proxy,
        config.pipeline.get_input_sink(),
        create_stream_cb,
    )?;

    Ok((Some(input_service), Some(config.pipeline.clone())))
}

pub(crate) async fn start(
    addr: &SocketAddr,
    client: &HttpClient,
    displays: &Vec<Display>,
    streaming_config: &StreamingSessionConfig,
    msg_proxy: MessageProxy,
) -> Result<StartStreamResult> {
    let streamer_ip = addr.ip().to_string();

    // Prepare video config
    let local_ip = host_service::hello(client).await?;

    let video_config =
        create_video_config(client, displays, &local_ip, &streaming_config.video).await?;

    // Prepare audio config
    let audio_config = if streaming_config.audio {
        Some(AudioConfig {
            ip: local_ip.clone(),
            port: RTP_AUDIO_PORT,
        })
    } else {
        None
    };

    // Start stream
    host_service::start_stream(
        client,
        &local_ip,
        &video_config,
        &audio_config,
        streaming_config.input.is_some(),
    )
    .await?;

    // Start Video/Audio playback
    let player = if video_config.is_some() || streaming_config.audio {
        let mut player = Player::new(msg_proxy.clone())?;
        player.play(video_config, audio_config)?;

        Some(crate::Player::Rtp(player))
    } else {
        None
    };

    // Start Inputs
    let (input_service, input_pipeline) = if let Some(config) = streaming_config.input.as_ref() {
        start_input(&streamer_ip, config, msg_proxy)?
    } else {
        (None, None)
    };

    Ok(StartStreamResult {
        player,
        input_service,
        input_pipeline,
    })
}
