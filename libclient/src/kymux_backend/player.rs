use log::{error, info, warn};

use crate::error::{Error, PlayerError, Result};
use crate::utils;
use crate::MessageProxy;

use super::{AudioConfig, VideoConfig};

pub(crate) struct Player {
    instance: vlc::Instance,
    video_mediaplayer: Option<vlc::MediaPlayer>,
    audio_mediaplayer: Option<vlc::MediaPlayer>,
    message_proxy: MessageProxy,
}

impl Player {
    pub(crate) fn new(message_proxy: MessageProxy) -> Result<Self> {
        let mut options = vec![
            "--0latency",
            "--low-delay",
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

    fn play_video(&mut self, config: VideoConfig) -> Result<()> {
        info!("Play video. URI: {}", config.uri);

        let media = vlc::Media::new_location(&config.uri).ok_or(PlayerError::MediaCreateFailed)?;
        let mp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mp.set_media(&media);

        utils::attach_window_to_player(&mp, config.window_handle)?;
        utils::plug_event_manager_to_player(&mp, self.message_proxy.clone())?;

        // Play
        mp.play().or(Err(PlayerError::PlayFailed))?;

        self.video_mediaplayer = Some(mp);

        Ok(())
    }

    fn play_audio(&mut self, config: AudioConfig) -> Result<()> {
        info!("Play audio. URI: {}", config.uri);

        let media = vlc::Media::new_location(&config.uri).ok_or(PlayerError::MediaCreateFailed)?;
        media.add_option(":clock-master=audio");

        let mp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mp.set_media(&media);

        utils::plug_event_manager_to_player(&mp, self.message_proxy.clone())?;

        // Play
        mp.play().or(Err(PlayerError::PlayFailed))?;

        self.audio_mediaplayer = Some(mp);

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
            let ret = self.play_video(video_config);
            if let Err(err) = ret {
                error!("Fail to start video playback: {:?}", err);
                return Err(err);
            }
        }

        // Audio playback
        if let Some(audio_config) = audio_config {
            let ret = self.play_audio(audio_config);
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
