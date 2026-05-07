// Project Kyber: player.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::collections::HashMap;
use std::sync::Arc;

use log::{error, info, warn};
use vlc::MediaPlayerAudioEx;
use vlc::MetricsEntry;

use super::window_handle::WindowHandle;

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("No audio player available")]
    NoAudioPlayer,
    #[error("Failed to create instance")]
    InstanceCreateFailed,
    #[error("Failed to create media")]
    MediaCreateFailed,
    #[error("Failed to create media player")]
    MediaPlayerCreateFailed,
    #[error("Failed to play")]
    PlayFailed,
    #[error("Already stopped")]
    AlreadyStopped,
    #[error("Failed to register event")]
    EvtRegisterFailed,
    #[error("Action failed")]
    ActionFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerMetric {
    FrameReceived,
    FrameDecoding,
    FrameDecoded,
    FrameSkipped,
    FramePrepared,
    FrameDisplayed,
}

#[derive(Clone, Debug)]
pub enum PlayerEvent {
    PlayerStarted,
    PlayerStopped,
    AudioVolumeChanged {
        volume: f32,
    },
    AudioMuteChanged {
        mute: bool,
    },
    Metric {
        source_id: u32,
        pts: u64,
        ts: i64,
        data: PlayerMetric,
    },
}

pub trait PlayerListener: Send + Sync {
    fn on_event(&self, event: PlayerEvent);
}

impl<F> PlayerListener for F
where
    F: Fn(PlayerEvent) + Send + Sync + 'static,
{
    fn on_event(&self, event: PlayerEvent) {
        self(event)
    }
}

#[allow(dead_code)]
fn setup_vlc_logger(instance: &vlc::Instance) {
    instance.set_log(
        |level: vlc::LogLevel, _log_ctx: vlc::Log, msg: std::borrow::Cow<str>| {
            let level = match level {
                vlc::LogLevel::Debug => log::Level::Debug,
                vlc::LogLevel::Notice => log::Level::Info,
                vlc::LogLevel::Warning => log::Level::Warn,
                vlc::LogLevel::Error => log::Level::Error,
            };

            log::log!(target: "vlc", level, "{msg}");
        },
    );
}

fn plug_event_manager_to_player(
    mp: &vlc::MediaPlayer,
    listener: Arc<dyn PlayerListener>,
) -> Result<(), PlayerError> {
    // Register to libvlc's event manager
    let event_manager = mp.event_manager();

    let listener2 = listener.clone();
    let cb = move |event, _| {
        let event = match event {
            vlc::Event::MediaPlayerPlaying => Some(PlayerEvent::PlayerStarted),
            vlc::Event::MediaPlayerStopped => Some(PlayerEvent::PlayerStopped),
            vlc::Event::MediaPlayerMuted => Some(PlayerEvent::AudioMuteChanged { mute: true }),
            vlc::Event::MediaPlayerUnmuted => Some(PlayerEvent::AudioMuteChanged { mute: false }),
            vlc::Event::MediaPlayerAudioVolume(volume) => {
                Some(PlayerEvent::AudioVolumeChanged { volume })
            }
            _ => None,
        };

        if let Some(event) = event {
            listener2.on_event(event);
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

pub struct VideoConfig {
    pub source_id: u32,
    pub software_decode: bool,
    pub window_handle: WindowHandle,
    pub uri: String,
    pub video_buffer: u32, // ms
}

pub struct AudioConfig {
    pub uri: String,
    pub audio_buffer: u32, // ms
}

pub struct Player {
    instance: vlc::Instance,
    video_mediaplayer: Vec<vlc::MediaPlayer>,
    audio_mediaplayer: Option<vlc::MediaPlayer>,
    listener: Arc<dyn PlayerListener>,
    metrics: bool,
}

impl Player {
    pub fn new(
        listener: Box<dyn PlayerListener>,
        video_buffer: Option<u32>,
        audio_buffer: Option<u32>,
        metrics: bool,
    ) -> Result<Self, PlayerError> {
        let listener = Arc::<dyn PlayerListener>::from(listener);

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
        #[cfg(not(target_os = "android"))]
        setup_vlc_logger(&instance);

        Ok(Self {
            instance,
            video_mediaplayer: vec![],
            audio_mediaplayer: None,
            listener,
            metrics,
        })
    }

    fn on_metrics(source_id: u32, map: HashMap<String, i64>, listener: &dyn PlayerListener) {
        let Some(pts) = map.get("pts").map(|&v| u64::try_from(v).unwrap()) else {
            warn!("Metrics without pts: ignoring");
            return;
        };
        if let Some(&ts) = map.get("received") {
            listener.on_event(PlayerEvent::Metric {
                source_id,
                pts,
                ts,
                data: PlayerMetric::FrameReceived,
            });
        }
        if let Some(&ts) = map.get("decoding") {
            listener.on_event(PlayerEvent::Metric {
                source_id,
                pts,
                ts,
                data: PlayerMetric::FrameDecoding,
            });
        }
        if let Some(&ts) = map.get("decoded") {
            listener.on_event(PlayerEvent::Metric {
                source_id,
                pts,
                ts,
                data: PlayerMetric::FrameDecoded,
            });
        }
        if let Some(&ts) = map.get("prepared") {
            listener.on_event(PlayerEvent::Metric {
                source_id,
                pts,
                ts,
                data: PlayerMetric::FramePrepared,
            });
        }
        if let Some(&ts) = map.get("displayed") {
            listener.on_event(PlayerEvent::Metric {
                source_id,
                pts,
                ts,
                data: PlayerMetric::FrameDisplayed,
            });
        }
        if let Some(&ts) = map.get("skipped") {
            listener.on_event(PlayerEvent::Metric {
                source_id,
                pts,
                ts,
                data: PlayerMetric::FrameSkipped,
            });
        }
    }

    fn metrics_to_map(entries: Vec<MetricsEntry>) -> HashMap<String, i64> {
        let mut map = HashMap::new();
        for MetricsEntry { key, value } in entries {
            map.insert(key, value);
        }
        map
    }

    fn play_video(&mut self, config: VideoConfig) -> Result<(), PlayerError> {
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
            let listener2 = self.listener.clone();
            mp.set_metrics_callback(move |entries| {
                let map = Self::metrics_to_map(entries);
                Self::on_metrics(source_id, map, &*listener2);
            });
        }

        match config.window_handle {
            #[cfg(target_os = "android")]
            WindowHandle::Android(handle) => {
                info!("Android");
                mp.set_android_context(handle as *mut libc::c_void);
            }
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
            #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
            WindowHandle::UiKit(handle) => {
                info!("ios ({})", handle);
                mp.set_nsobject(handle as _);
            }
        }

        plug_event_manager_to_player(&mp, self.listener.clone())?;

        // Play
        mp.play().or(Err(PlayerError::PlayFailed))?;

        self.video_mediaplayer.push(mp);

        Ok(())
    }

    fn play_audio(&mut self, config: AudioConfig) -> Result<(), PlayerError> {
        info!("Play audio. URI: {}", config.uri);

        let media = vlc::Media::new_location(&config.uri).ok_or(PlayerError::MediaCreateFailed)?;
        media.add_option(":clock-master=audio");

        let mp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mp.set_media(&media);

        plug_event_manager_to_player(&mp, self.listener.clone())?;

        // Play
        mp.play().or(Err(PlayerError::PlayFailed))?;

        self.audio_mediaplayer = Some(mp);

        Ok(())
    }

    pub fn play(
        &mut self,
        video_config: Option<Vec<VideoConfig>>,
        audio_config: Option<AudioConfig>,
    ) -> Result<(), PlayerError> {
        if video_config.is_none() && audio_config.is_none() {
            error!("play() rejected because no video and audio conf have been provided");
            return Err(PlayerError::PlayFailed);
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

    async fn mdp_stop_async(mdp: &vlc::MediaPlayer) -> Result<(), PlayerError> {
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

    pub async fn stop_async(&mut self) -> Result<(), PlayerError> {
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

    pub fn audio_set_volume(&self, volume: f32) -> Result<(), PlayerError> {
        let audio_mediaplayer = self
            .audio_mediaplayer
            .as_ref()
            .ok_or(PlayerError::NoAudioPlayer)?;

        let ret = audio_mediaplayer.set_volume((volume * 100.0) as i32);
        if ret.is_err() {
            warn!("Failed to set audio volume");
            return Err(PlayerError::ActionFailed);
        }

        Ok(())
    }

    pub fn audio_set_mute(&self, mute: bool) -> Result<(), PlayerError> {
        let audio_mediaplayer = self
            .audio_mediaplayer
            .as_ref()
            .ok_or(PlayerError::NoAudioPlayer)?;

        audio_mediaplayer.set_mute(mute);

        Ok(())
    }
}
