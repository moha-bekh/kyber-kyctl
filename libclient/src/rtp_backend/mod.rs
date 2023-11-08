pub mod host_service;

use std::env;
use std::fs;

use async_trait::async_trait;
use log::{debug, info};

use libkynput_rs::Pipeline;

use crate::http::HttpClient;
use crate::{get_display_id, input_service, player};
use crate::{
    BackendDriver, ConnectionParams, Display, Error, InputSessionConfig, MessageProxy,
    PlayerConfig, Result, StreamingSessionConfig, VideoSessionConfig,
};

const RTP_VIDEO_PORT: u16 = 30000;

fn create_video_sdp_file(streamer: &str, video_port: u16, sdp_param: &str) -> Result<String> {
    let sdp_path = env::temp_dir().join("video.sdp");

    let sdp = format!(
        "v=0\n\
        o=- 0 0 IN IP4 {streamer_ip}\n\
        s=No Name\n\
        c=IN IP4 {streamer_ip}\n\
        t=0 0\n\
        a=tool:libavformat 58.45.100\n\
        m=video {video_port} RTP/AVP 96\n\
        a=rtpmap:96 H264/90000\n\
        {sdp_param}\n",
        streamer_ip = streamer,
        video_port = video_port,
        sdp_param = sdp_param,
    );

    debug!("Write video SDP to {:?}", sdp_path);
    fs::write(&sdp_path, sdp)?;

    Ok(format!(
        "file:///{}",
        sdp_path.to_str().ok_or(Error::InvalidUnicodeStr)?
    ))
}

fn create_audio_sdp_file(streamer: &str, audio_port: u16) -> Result<String> {
    let sdp_path = env::temp_dir().join("audio.sdp");

    let sdp = format!(
        "v=0\n\
        o=- 0 0 IN IP4 {streamer_ip}\n\
        s=No Name\n\
        c=IN IP4 {streamer_ip}\n\
        t=0 0\n\
        a=tool:libavformat 59.16.100\n\
        m=audio {audio_port} RTP/AVP 97\n\
        b=AS:128\n\
        a=rtpmap:97 opus/48000/2\n\
        a=fmtp:97 sprop-stereo=1\n",
        streamer_ip = streamer,
        audio_port = audio_port,
    );

    debug!("Write audio SDP to {:?}", sdp_path);
    fs::write(&sdp_path, sdp)?;

    Ok(format!(
        "file:///{}",
        sdp_path.to_str().ok_or(Error::InvalidUnicodeStr)?
    ))
}

// RTP seems to use multiple port for a session. Fix a large offset to get
// a safe port.
const RTP_AUDIO_PORT: u16 = RTP_VIDEO_PORT + 10;

struct HostVideoConfig {
    display_id: u32,
    yuv444: bool,
    port: u16,
}

struct HostAudioConfig {
    port: u16,
}

struct InputService {
    client: input_service::Client,
    pipeline: Pipeline,
}

pub(crate) struct RtpBackend {
    player: Option<player::Player>,
    input_service: Option<InputService>,
}

async fn create_video_config(
    client: &HttpClient,
    displays: &Vec<Display>,
    ip: &str,
    config: &Option<VideoSessionConfig>,
    player_config: &PlayerConfig,
) -> Result<(Option<HostVideoConfig>, Option<player::VideoConfig>)> {
    let Some(config) = config.as_ref() else {
        return Ok((None, None));
    };

    let Some(window_handle) = player_config.window_handle else {
        return Ok((None, None));
    };

    // Create Host config
    let display_id = get_display_id(displays, config).id;

    let host_videoconfig = HostVideoConfig {
        display_id,
        yuv444: config.yuv444,
        port: RTP_VIDEO_PORT,
    };

    // Create vlc config
    let sdp_param = host_service::generate_sdp(client, config.yuv444, display_id).await?;
    let sdp_path = create_video_sdp_file(ip, RTP_VIDEO_PORT, &sdp_param)?;

    let vlc_videoconfig = player::VideoConfig {
        uri: sdp_path,
        window_handle,
    };

    Ok((Some(host_videoconfig), Some(vlc_videoconfig)))
}

fn start_input(
    streamer_ip: &str,
    config: &InputSessionConfig,
    msg_proxy: MessageProxy,
) -> Result<Option<InputService>> {
    let create_stream_cb = |_target, msg_sink| libkynput_rs::tcp::new_client(streamer_ip, msg_sink);

    let client = input_service::Client::new(
        msg_proxy,
        config.pipeline.get_input_sink(),
        create_stream_cb,
    )?;

    Ok(Some(InputService {
        client,
        pipeline: config.pipeline.clone(),
    }))
}

impl RtpBackend {
    pub(crate) async fn new(
        conn_params: &ConnectionParams,
        client: &HttpClient,
        displays: &Vec<Display>,
        player_config: &PlayerConfig,
        streaming_config: &StreamingSessionConfig,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        // Prepare video config
        let local_ip = host_service::hello(client).await?;

        let (host_videoconfig, vlc_videoconfig) = create_video_config(
            client,
            displays,
            &local_ip,
            &streaming_config.video,
            player_config,
        )
        .await?;

        // Prepare audio config
        let (host_audioconfig, vlc_audioconfig) = if streaming_config.audio {
            let host_config = HostAudioConfig {
                port: RTP_AUDIO_PORT,
            };

            let uri = create_audio_sdp_file(&local_ip, RTP_AUDIO_PORT)?;
            let vlc_config = player::AudioConfig { uri };

            (Some(host_config), Some(vlc_config))
        } else {
            (None, None)
        };

        // Start stream
        host_service::start_stream(
            client,
            &local_ip,
            streaming_config.bitrate,
            &host_videoconfig,
            &host_audioconfig,
            streaming_config.input.is_some(),
        )
        .await?;

        // Start Video/Audio playback
        let player = if vlc_videoconfig.is_some() || vlc_audioconfig.is_some() {
            let mut player = player::Player::new(player_config.software_decode, msg_proxy.clone())?;
            player.play(vlc_videoconfig, vlc_audioconfig)?;

            Some(player)
        } else {
            None
        };

        // Start Inputs
        let input_service = if let Some(config) = streaming_config.input.as_ref() {
            start_input(&conn_params.addr.ip().to_string(), config, msg_proxy)?
        } else {
            None
        };

        Ok(Self {
            player,
            input_service,
        })
    }
}

#[async_trait]
impl BackendDriver for RtpBackend {
    async fn input_service_connected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service
            .pipeline
            .plug_network(&input_service.client.get_tx_consumer())?;
        input_service.pipeline.start()?;

        Ok(())
    }

    async fn input_service_disconnected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;
        input_service.pipeline.stop()?;

        self.input_service = None;

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(mut player) = self.player.take() {
            info!("Stop player");
            player.stop_async().await?;
            info!("Player stopped");
        }

        if let Some(mut input_service) = self.input_service.take() {
            info!("Stop input service");
            input_service.client.stop().await?;
            info!("Input service stopped");
        }

        Ok(())
    }
}
