use std::borrow::Cow;
use std::collections::HashMap;

use log::{error, info, warn};
use vlc::MediaPlayerAudioEx;
use vlc::MetricsEntry;

use crate::error::{Error, PlayerError, Result};
use crate::metrics::{Metric, VideoMetric, VideoMetricData};
use crate::platform::WindowHandle;
use crate::{Message, MessageSender};

pub(crate) fn plug_event_manager_to_player(
    mp: &vlc::MediaPlayer,
    message_sender: MessageSender,
) -> Result<()> {
    // Register to libvlc's event manager
    let event_manager = mp.event_manager();

    let cb = move |event, _| {
        let msg = match event {
            vlc::Event::MediaPlayerPlaying => Some(Message::PlayerStarted),
            vlc::Event::MediaPlayerStopped => Some(Message::PlayerStopped),
            vlc::Event::MediaPlayerMuted => Some(Message::AudioMuteChanged { mute: true }),
            vlc::Event::MediaPlayerUnmuted => Some(Message::AudioMuteChanged { mute: false }),
            vlc::Event::MediaPlayerAudioVolume(volume) => {
                Some(Message::AudioVolumeChanged { volume })
            }
            _ => None,
        };

        if let Some(msg) = msg {
            let ret = message_sender.send(msg);
            if ret.is_err() {
                warn!("Failed to send vlc::Event to MessageSender");
            }
        }
    };

    let events = &[
        vlc::EventType::MediaPlayerPlaying,
        vlc::EventType::MediaPlayerMuted,
        vlc::EventType::MediaPlayerUnmuted,
        vlc::EventType::MediaPlayerAudioVolume,
    ];

    for e in events {
        event_manager
            .attach(*e, cb.clone())
            .map_err(|_| PlayerError::EvtRegisterFailed)?;
    }

    Ok(())
}

pub(crate) struct VideoConfig {
    pub(crate) source_id: u32,
    pub(crate) software_decode: bool,
    pub(crate) window_handle: WindowHandle,
    pub(crate) uri: String,
    pub(crate) video_buffer: u32, // ms
}

pub(crate) struct AudioConfig {
    pub(crate) uri: String,
    pub(crate) audio_buffer: u32, // ms
}

pub(crate) struct Player {
    instance: vlc::Instance,
    video_mediaplayer: Vec<vlc::MediaPlayer>,
    audio_mediaplayer: Option<vlc::MediaPlayer>,
    message_sender: MessageSender,
    metrics: bool,
}

impl Player {
    pub(crate) fn new(
        message_sender: MessageSender,
        video_buffer: Option<u32>,
        audio_buffer: Option<u32>,
        metrics: bool,
    ) -> Result<Self> {
        let mut options = vec![
            "--0latency".to_string(),
            "--low-delay".to_string(),
            "--network-caching=0".to_string(),
            "--video-title=Kyber Client".to_string(),
            "--no-spu".to_string(),
            "--no-osd".to_string(),
        ];

        if let Some(video_buffer) = video_buffer {
            options.push(format!("--video-buffer={video_buffer}"));
        }

        if let Some(audio_buffer) = audio_buffer {
            options.push(format!("--audio-buffer={audio_buffer}"));
        }

        let instance =
            vlc::Instance::with_args(Some(options)).ok_or(PlayerError::InstanceCreateFailed)?;

        instance.set_log(|level: vlc::LogLevel, _log_ctx: vlc::Log, msg: Cow<str>| {
            let level = match level {
                vlc::LogLevel::Debug => log::Level::Debug,
                vlc::LogLevel::Notice => log::Level::Info,
                vlc::LogLevel::Warning => log::Level::Warn,
                vlc::LogLevel::Error => log::Level::Error,
            };

            log::log!(target: "vlc", level, "{msg}");
        });

        Ok(Self {
            instance,
            video_mediaplayer: vec![],
            audio_mediaplayer: None,
            message_sender,
            metrics,
        })
    }

    fn on_metrics(source_id: u32, map: HashMap<String, i64>, message_sender: &MessageSender) {
        let Some(pts) = map.get("pts").map(|&v| u64::try_from(v).unwrap()) else {
            warn!("Metrics without pts: ignoring");
            return;
        };
        if let Some(&ts) = map.get("received") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameReceived,
            });
            Self::send_metric(message_sender, metric);
        }
        if let Some(&ts) = map.get("decoding") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameDecoding,
            });
            Self::send_metric(message_sender, metric);
        }
        if let Some(&ts) = map.get("decoded") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameDecoded,
            });
            Self::send_metric(message_sender, metric);
        }
        if let Some(&ts) = map.get("prepared") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FramePrepared,
            });
            Self::send_metric(message_sender, metric);
        }
        if let Some(&ts) = map.get("displayed") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameDisplayed,
            });
            Self::send_metric(message_sender, metric);
        }
        if let Some(&ts) = map.get("skipped") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameSkipped,
            });
            Self::send_metric(message_sender, metric);
        }
    }

    fn metrics_to_map(entries: Vec<MetricsEntry>) -> HashMap<String, i64> {
        let mut map = HashMap::new();
        for MetricsEntry { key, value } in entries {
            map.insert(key, value);
        }
        map
    }

    fn send_metric(message_sender: &MessageSender, metric: Metric) {
        let msg = Message::Metric(metric);
        if let Err(err) = message_sender.send(msg) {
            warn!("Failed to send metric to MessageSender: {err:?}");
        }
    }

    fn play_video(&mut self, config: VideoConfig) -> Result<()> {
        info!("Play video. URI: {}", config.uri);

        let media = vlc::Media::new_location(&config.uri).ok_or(PlayerError::MediaCreateFailed)?;
        if config.software_decode || std::env::var("KYBER_SOFTWARE_DECODE").is_ok() {
            info!("Enable software decoding");
            media.add_option(":no-hw-dec");
        }

        let mp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mp.set_media(&media);

        if self.metrics {
            // Note: VLC metrics are disabled if the metrics callback is not set
            let source_id = config.source_id;
            let message_sender2 = self.message_sender.clone();
            mp.set_metrics_callback(move |entries| {
                let map = Self::metrics_to_map(entries);
                Self::on_metrics(source_id, map, &message_sender2);
            });
        }

        match config.window_handle {
            #[cfg(target_os = "linux")]
            WindowHandle::Xlib(handle) => {
                info!("Xlib");
                mp.set_xwindow(handle as u32);
            }
            #[cfg(target_os = "macos")]
            WindowHandle::AppKit(handle) => {
                info!("macos ({})", handle);
                mp.set_nsobject(handle as _);
            }
            #[cfg(target_os = "windows")]
            WindowHandle::Win32(handle) => {
                info!("Windows");
                mp.set_hwnd(handle as _);
            }
        }

        plug_event_manager_to_player(&mp, self.message_sender.clone())?;

        // Play
        mp.play().or(Err(PlayerError::PlayFailed))?;

        self.video_mediaplayer.push(mp);

        Ok(())
    }

    fn play_audio(&mut self, config: AudioConfig) -> Result<()> {
        info!("Play audio. URI: {}", config.uri);

        let media = vlc::Media::new_location(&config.uri).ok_or(PlayerError::MediaCreateFailed)?;
        media.add_option(":clock-master=audio");

        let mp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mp.set_media(&media);

        plug_event_manager_to_player(&mp, self.message_sender.clone())?;

        // Play
        mp.play().or(Err(PlayerError::PlayFailed))?;

        self.audio_mediaplayer = Some(mp);

        Ok(())
    }

    pub(crate) fn play(
        &mut self,
        video_config: Option<Vec<VideoConfig>>,
        audio_config: Option<AudioConfig>,
    ) -> Result<()> {
        if video_config.is_none() && audio_config.is_none() {
            error!("play() rejected because no video and audio conf have been provided");
            return Err(Error::Rejected);
        }

        // Video playback
        if let Some(video_config) = video_config {
            for config in video_config {
                let ret = self.play_video(config);
                if let Err(err) = ret {
                    error!("Fail to start video playback: {:?}", err);
                    return Err(err);
                }
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
                let ret = tx.blocking_send(());
                if ret.is_err() {
                    warn!("Failed to send vlc::Event::MediaPlayerStopped ack");
                }
            }
            _ => {
                warn!("Unexpected event during stop");
            }
        };

        let event_manager = mdp.event_manager();
        event_manager
            .attach(vlc::EventType::MediaPlayerStopped, cb)
            .map_err(|_| PlayerError::EvtRegisterFailed)?;

        let ret = mdp.stop();
        if ret.is_ok() {
            let _ = rx.recv().await;
        }

        Ok(())
    }

    pub(crate) async fn stop_async(&mut self) -> Result<()> {
        if let Some(audio_mediaplayer) = self.audio_mediaplayer.as_mut() {
            info!("Stop audio player");
            let ret = Self::mdp_stop_async(audio_mediaplayer).await;
            if ret.is_err() {
                warn!("Failed to stop audio player");
            }
        }

        info!("Stop video player");
        for video_mediaplayer in self.video_mediaplayer.iter() {
            Self::mdp_stop_async(video_mediaplayer).await?;
        }

        Ok(())
    }

    pub(crate) fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let audio_mediaplayer = self
            .audio_mediaplayer
            .as_ref()
            .ok_or(Error::NoAudioPlayer)?;

        let ret = audio_mediaplayer.set_volume((volume * 100.0) as i32);
        if ret.is_err() {
            warn!("Failed to set audio volume");
            return Err(Error::Rejected);
        }

        Ok(())
    }

    pub(crate) fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let audio_mediaplayer = self
            .audio_mediaplayer
            .as_ref()
            .ok_or(Error::NoAudioPlayer)?;

        audio_mediaplayer.set_mute(mute);

        Ok(())
    }
}
