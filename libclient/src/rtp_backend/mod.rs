pub mod host_service;

use std::env;
use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use log::{debug, info};

use crate::http::HttpClient;
use crate::platform::player;
use crate::platform::{InputClient, InputPipeline};
use crate::{BackendDriver, ConnectionParams, Error, MessageProxy, Result, StreamingSessionConfig};

const RTP_VIDEO_PORT: u16 = 30000;

fn get_tmp_folder() -> PathBuf {
    if let Ok(path) = env::var("KYBER_CACHEDIR") {
        PathBuf::from(path)
    } else {
        env::temp_dir()
    }
}

fn create_video_sdp_file(streamer: &str, video_port: u16, sdp_param: &str) -> Result<String> {
    let sdp_path = get_tmp_folder().join("video.sdp");

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
    let sdp_path = get_tmp_folder().join("audio.sdp");

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
    intra_refresh: Option<bool>,
}

struct HostAudioConfig {
    port: u16,
}

struct InputService {
    client: InputClient,
    pipeline: InputPipeline,
}

pub(crate) struct RtpBackendConfig<'a> {
    pub(crate) conn_params: &'a ConnectionParams,
    pub(crate) http_client: &'a HttpClient,
    pub(crate) streaming_config: StreamingSessionConfig,
    pub(crate) msg_proxy: MessageProxy,
}

pub(crate) struct RtpBackend {
    player: Option<player::Player>,
    input_service: Option<InputService>,
}

impl RtpBackend {
    pub(crate) async fn new(backend_config: RtpBackendConfig<'_>) -> Result<Self> {
        // Prepare video config
        let local_ip = host_service::hello(backend_config.http_client).await?;

        let (host_videoconfig, vlc_videoconfig) =
            Self::create_video_config(&backend_config, &local_ip).await?;

        // Prepare audio config
        let (host_audioconfig, vlc_audioconfig) =
            Self::create_audio_config(&backend_config, &local_ip).await?;

        // Start stream
        host_service::start_stream(
            backend_config.http_client,
            &local_ip,
            backend_config.streaming_config.bitrate,
            &host_videoconfig,
            &host_audioconfig,
            &backend_config.streaming_config.input,
        )
        .await?;

        // Start Video/Audio playback
        let player = if vlc_videoconfig.is_some() || vlc_audioconfig.is_some() {
            let vlc_videoconfig = vlc_videoconfig.map(|video_config| vec![video_config]);
            let audio_buffer = vlc_audioconfig.as_ref().map(|config| config.audio_buffer);

            let mut player = player::Player::new(backend_config.msg_proxy.clone(), audio_buffer)?;
            player.play(vlc_videoconfig, vlc_audioconfig)?;

            Some(player)
        } else {
            None
        };

        // Start Inputs
        let input_service = Self::start_input(&backend_config)?;

        Ok(Self {
            player,
            input_service,
        })
    }

    async fn create_video_config(
        backend_config: &RtpBackendConfig<'_>,
        local_ip: &str,
    ) -> Result<(Option<HostVideoConfig>, Option<player::VideoConfig>)> {
        let video_config = &backend_config.streaming_config.video;
        if video_config.is_empty() {
            return Ok((None, None));
        }

        let video_config = &video_config[0];

        // Create Host config
        let host_videoconfig = HostVideoConfig {
            display_id: video_config.display_id,
            yuv444: video_config.yuv444,
            port: RTP_VIDEO_PORT,
            intra_refresh: video_config.intra_refresh,
        };

        // Create vlc config
        let sdp_param = host_service::generate_sdp(
            backend_config.http_client,
            video_config.yuv444,
            video_config.display_id,
        )
        .await?;
        let sdp_path = create_video_sdp_file(local_ip, RTP_VIDEO_PORT, &sdp_param)?;

        let vlc_videoconfig = player::VideoConfig {
            uri: sdp_path,
            software_decode: video_config.player.software_decode,
            window_handle: video_config.player.window_handle,
        };

        Ok((Some(host_videoconfig), Some(vlc_videoconfig)))
    }

    async fn create_audio_config(
        backend_config: &RtpBackendConfig<'_>,
        local_ip: &str,
    ) -> Result<(Option<HostAudioConfig>, Option<player::AudioConfig>)> {
        let Some(audio_config) = &backend_config.streaming_config.audio else {
            return Ok((None, None));
        };

        let host_config = HostAudioConfig {
            port: RTP_AUDIO_PORT,
        };

        let uri = create_audio_sdp_file(local_ip, RTP_AUDIO_PORT)?;
        let audio_buffer = audio_config.audio_buffer();
        let vlc_config = player::AudioConfig { uri, audio_buffer };

        Ok((Some(host_config), Some(vlc_config)))
    }

    fn start_input(backend_config: &RtpBackendConfig<'_>) -> Result<Option<InputService>> {
        let Some(config) = backend_config.streaming_config.input.as_ref() else {
            return Ok(None);
        };

        let streamer_ip = backend_config.conn_params.addr.ip().to_string();

        let create_stream_cb =
            |_target, msg_sink| libkynput_rs::tcp::new_client(&streamer_ip, msg_sink);

        let client = InputClient::new(
            backend_config.msg_proxy.clone(),
            config.pipeline.get_input_sink(),
            create_stream_cb,
        )?;

        Ok(Some(InputService {
            client,
            pipeline: config.pipeline.clone(),
        }))
    }
}

#[async_trait]
impl BackendDriver for RtpBackend {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let player = self.player.as_ref().ok_or(Error::NoAudioPlayer)?;

        player.audio_set_volume(volume)?;

        Ok(())
    }

    fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let player = self.player.as_ref().ok_or(Error::NoAudioPlayer)?;

        player.audio_set_mute(mute)?;

        Ok(())
    }

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
