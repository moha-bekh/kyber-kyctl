use raw_window_handle::RawWindowHandle;

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub enum WindowHandle {
    #[cfg(target_os = "linux")]
    Xlib(libc::c_ulong),
    #[cfg(target_os = "windows")]
    Win32(u64),
}

impl From<RawWindowHandle> for WindowHandle {
    fn from(handle: RawWindowHandle) -> WindowHandle {
        match handle {
            #[cfg(target_os = "linux")]
            RawWindowHandle::Xlib(handle) => WindowHandle::Xlib(handle.window),
            #[cfg(target_os = "windows")]
            RawWindowHandle::Win32(handle) => {
                let handle: u64 = handle.hwnd as u64;

                WindowHandle::Win32(handle)
            }
            _ => {
                panic!("Unexpected handle type");
            }
        }
    }
}
