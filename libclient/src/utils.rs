use log::info;

use crate::error::PlayerError;
use crate::{Message, MessageProxy, Result, WindowHandle};

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
