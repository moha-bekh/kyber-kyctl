use std::borrow::Cow;

use log::info;

use crate::error::PlayerError;
use crate::{Message, MessageProxy, Result, WindowHandle};

fn vlc_log_cb(level: vlc::LogLevel, _log_ctx: vlc::Log, msg: Cow<str>) {
    let level = match level {
        vlc::LogLevel::Debug => log::Level::Debug,
        vlc::LogLevel::Notice => log::Level::Info,
        vlc::LogLevel::Warning => log::Level::Warn,
        vlc::LogLevel::Error => log::Level::Error,
    };

    log::log!(target: "vlc", level, "{msg}");
}

pub(crate) fn plug_vlc_logger(instance: &vlc::Instance) {
    instance.set_log(vlc_log_cb);
}

pub(crate) fn attach_window_to_player(
    mp: &vlc::MediaPlayer,
    window_handle: WindowHandle,
) -> Result<()> {
    match window_handle {
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

    Ok(())
}

pub(crate) fn plug_event_manager_to_player(
    mp: &vlc::MediaPlayer,
    message_proxy: MessageProxy,
) -> Result<()> {
    // Register to libvlc's event manager
    let event_manager = mp.event_manager();

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

    Ok(())
}
