use log::{debug, error, info, warn};
use std::path::PathBuf;
use std::{env, fs};

use crate::error::{Error, PlayerError, Result};
use crate::utils;
use crate::MessageProxy;

use super::{AudioConfig, VideoConfig};

fn create_video_sdp_file(streamer: &str, video_port: u16, sdp_param: &str) -> Result<PathBuf> {
    // Prepare SDP
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

    Ok(sdp_path)
}

fn create_audio_sdp_file(streamer: &str, audio_port: u16) -> Result<PathBuf> {
    // Prepare SDP
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

    Ok(sdp_path)
}

pub struct Player {
    instance: vlc::Instance,
    video_mediaplayer: Option<vlc::MediaPlayer>,
    audio_mediaplayer: Option<vlc::MediaPlayer>,
    message_proxy: MessageProxy,
}

impl Player {
    pub fn new(message_proxy: MessageProxy) -> Result<Self> {
        let mut options = vec![
            "--0latency",
            "--network-caching=0",
            "--video-title=Kyber Client",
        ];

        if std::env::var("KYBER_SOFTWARE_DECODE").is_ok() {
            options.append(&mut vec!["--no-hw-dec"]);
        }

        let instance =
            vlc::Instance::with_args(Some(options.iter().map(|&s| s.to_string()).collect()))
                .ok_or(PlayerError::InstanceCreateFailed)?;

        utils::plug_vlc_logger(&instance);

        Ok(Self {
            instance,
            video_mediaplayer: None,
            audio_mediaplayer: None,
            message_proxy,
        })
    }

    fn play_video(&mut self, config: &VideoConfig) -> Result<()> {
        // Create player
        let sdp_path = create_video_sdp_file(&config.ip, config.port, &config.sdp_param)?;

        let mrl = format!(
            "file:///{}",
            sdp_path.to_str().ok_or(Error::InvalidUnicodeStr)?
        );

        let md = vlc::Media::new_location(&mrl).ok_or(PlayerError::MediaCreateFailed)?;

        let mdp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mdp.set_media(&md);

        utils::attach_window_to_player(&mdp, config.window_handle)?;
        utils::plug_event_manager_to_player(&mdp, self.message_proxy.clone())?;

        // Play
        mdp.play().or(Err(PlayerError::PlayFailed))?;

        self.video_mediaplayer = Some(mdp);

        Ok(())
    }

    fn play_audio(&mut self, config: &AudioConfig) -> Result<()> {
        let sdp_path = create_audio_sdp_file(&config.ip, config.port)?;

        let mrl = format!(
            "file:///{}",
            sdp_path.to_str().ok_or(Error::InvalidUnicodeStr)?
        );

        let md = vlc::Media::new_location(&mrl).ok_or(PlayerError::MediaCreateFailed)?;

        let mdp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mdp.set_media(&md);

        utils::plug_event_manager_to_player(&mdp, self.message_proxy.clone())?;

        // Play
        mdp.play().or(Err(PlayerError::PlayFailed))?;

        self.audio_mediaplayer = Some(mdp);

        Ok(())
    }

    pub(super) fn play(
        &mut self,
        video_config: Option<VideoConfig>,
        audio_config: Option<AudioConfig>,
    ) -> Result<()> {
        if video_config.is_none() && audio_config.is_none() {
            error!("play() rejected because no video and audio conf have been provided");
            return Err(Error::Rejected);
        }

        // Video playback
        if let Some(video_config) = video_config {
            info!("Play video from {}:{}", video_config.ip, video_config.port);

            let ret = self.play_video(&video_config);
            if let Err(err) = ret {
                error!("Fail to start video playback: {:?}", err);
                return Err(err);
            }
        }

        // Audio playback
        if let Some(audio_config) = audio_config {
            info!("Play audio from {}:{}", audio_config.ip, audio_config.port);

            let ret = self.play_audio(&audio_config);
            if let Err(err) = ret {
                error!("Fail to start audio playback: {:?}", err);
                return Err(err);
            }
        }

        Ok(())
    }

    async fn mdp_stop_async(mdp: &vlc::MediaPlayer) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        let cb = move |event, _| match event {
            vlc::Event::MediaPlayerStopped => {
                tx.blocking_send(()).unwrap();
            }
            _ => {
                warn!("Unexpected event during stop");
            }
        };

        let event_manager = mdp.event_manager();
        event_manager
            .attach(vlc::EventType::MediaPlayerStopped, cb)
            .map_err(|_| PlayerError::EvtRegisterFailed)?;

        mdp.stop();
        let _ = rx.recv().await;

        Ok(())
    }

    pub async fn stop_async(&mut self) -> Result<()> {
        if let Some(audio_mediaplayer) = self.audio_mediaplayer.as_mut() {
            info!("Stop audio player");
            let ret = Self::mdp_stop_async(audio_mediaplayer).await;
            if ret.is_err() {
                warn!("Failed to stop audio player");
            }
        }

        info!("Stop video player");
        let video_mediaplayer = self
            .video_mediaplayer
            .as_mut()
            .ok_or(PlayerError::AlreadyStopped)?;
        Self::mdp_stop_async(video_mediaplayer).await?;

        Ok(())
    }
}
