use log::{debug, info, warn};
use std::path::PathBuf;
use std::{env, fs};

use crate::error::{Error, PlayerError, Result};

use super::window_handle::WindowHandle;
use super::{Message, MessageProxy};

fn create_sdp_file(streamer: &str, video_port: u16, sdp_param: String) -> Result<PathBuf> {
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

    debug!("Write SDP to {:?}", sdp_path);
    fs::write(&sdp_path, sdp)?;

    Ok(sdp_path)
}

pub struct Player {
    instance: vlc::Instance,
    mediaplayer: Option<vlc::MediaPlayer>,
    window_handle: WindowHandle,
    message_proxy: MessageProxy,
}

impl Player {
    pub fn new(window_handle: WindowHandle, message_proxy: MessageProxy) -> Result<Self> {
        let mut options = vec![
            "--0latency",
            "--network-caching=0",
            "--video-title=Kyber Client",
        ];

        if std::env::var("KYBER_SOFTWARE_DECODE").is_ok() {
            options.append(&mut vec!["--no-hw-dec"]);
        }

        if std::env::var("KYBER_VLC_VERBOSE").is_ok() {
            options.append(&mut vec!["-vv"]);
        }

        let instance =
            vlc::Instance::with_args(Some(options.iter().map(|&s| s.to_string()).collect()))
                .ok_or(PlayerError::InstanceCreateFailed)?;

        Ok(Self {
            instance,
            mediaplayer: None,
            window_handle,
            message_proxy,
        })
    }

    pub fn play(&mut self, ip: &str, video_port: u16, sdp_param: String) -> Result<()> {
        // Create player
        let sdp_path = create_sdp_file(ip, video_port, sdp_param)?;

        let mrl = format!(
            "file:///{}",
            sdp_path.to_str().ok_or(Error::InvalidUnicodeStr)?
        );

        let md =
            vlc::Media::new_location(&self.instance, &mrl).ok_or(PlayerError::MediaCreateFailed)?;

        let mdp =
            vlc::MediaPlayer::new(&self.instance).ok_or(PlayerError::MediaPlayerCreateFailed)?;
        mdp.set_media(&md);

        // Attach Window
        match self.window_handle {
            #[cfg(target_os = "linux")]
            WindowHandle::Xlib(handle) => {
                info!("Xlib");
                mdp.set_xwindow(handle as u32);
            }
            #[cfg(target_os = "windows")]
            WindowHandle::Win32(handle) => {
                info!("Windows");
                mdp.set_hwnd(unsafe { std::mem::transmute(handle) });
            }
        }

        // Register to libvlc's event manager
        let event_manager = mdp.event_manager();

        let message_proxy = self.message_proxy.clone();
        let cb = move |event, _| {
            let msg = match event {
                vlc::Event::MediaPlayerPlaying => Some(Message::PlayerStarted),
                vlc::Event::MediaPlayerStopped => Some(Message::PlayerStopped),
                _ => None,
            };

            if let Some(msg) = msg {
                message_proxy.send(msg).unwrap();
            }
        };

        event_manager
            .attach(vlc::EventType::MediaPlayerPlaying, cb)
            .map_err(|_| PlayerError::EvtRegisterFailed)?;

        // Play
        mdp.play().or(Err(PlayerError::PlayFailed))?;

        self.mediaplayer = Some(mdp);

        Ok(())
    }

    pub async fn stop_async(&mut self) -> Result<()> {
        let mdp = self
            .mediaplayer
            .as_mut()
            .ok_or(PlayerError::AlreadyStopped)?;

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
}
