use raw_window_handle::RawWindowHandle;

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum WindowHandle {
    #[cfg(target_os = "linux")]
    Xlib(libc::c_ulong),
    #[cfg(target_os = "macos")]
    AppKit(u64),
    #[cfg(target_os = "windows")]
    Win32(u64),
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
    UiKit(u64),
}

impl From<RawWindowHandle> for WindowHandle {
    fn from(handle: RawWindowHandle) -> WindowHandle {
        match handle {
            #[cfg(target_os = "linux")]
            RawWindowHandle::Xlib(handle) => WindowHandle::Xlib(handle.window),
            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(handle) => {
                let handle: u64 = handle.ns_view as u64;

                WindowHandle::AppKit(handle)
            }
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
