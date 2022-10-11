use std::ffi::{CStr, CString};
use std::os::raw::c_void;

use libclient_sys as sys;
use libkynput_rs::Pipeline;

#[derive(Debug)]
pub enum Error {
    Generic,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug)]
pub enum StreamingProtocol {
    Rtp,
    Kymux,
}

impl StreamingProtocol {
    pub fn from_u32(u: sys::libclient_StreamingProtocol) -> Option<Self> {
        match u {
            sys::libclient_StreamingProtocol_LIBCLIENT_STREAMING_PROTOCOL_RTP => Some(Self::Rtp),
            sys::libclient_StreamingProtocol_LIBCLIENT_STREAMING_PROTOCOL_KYMUX => {
                Some(Self::Kymux)
            }
            _ => None,
        }
    }

    fn to_u32(self) -> u32 {
        match self {
            Self::Rtp => sys::libclient_StreamingProtocol_LIBCLIENT_STREAMING_PROTOCOL_RTP,
            Self::Kymux => sys::libclient_StreamingProtocol_LIBCLIENT_STREAMING_PROTOCOL_KYMUX,
        }
    }
}

pub struct WindowHandle {
    pub ptr: *mut sys::libclient_WindowHandle,
}

impl WindowHandle {
    #[cfg(target_os = "linux")]
    pub fn from_x11(handle: u64) -> Self {
        Self {
            ptr: unsafe { sys::libclient_windowshandle_from_x11(handle) },
        }
    }

    #[cfg(target_os = "macos")]
    pub fn from_ns_view(handle: u64) -> Self {
        Self {
            ptr: unsafe { sys::libclient_windowshandle_from_ns_view(handle) },
        }
    }

    #[cfg(target_os = "windows")]
    pub fn from_hwnd(hwnd: u64) -> Self {
        Self {
            ptr: unsafe { sys::libclient_windowshandle_from_hwnd(hwnd) },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Stopped,
}

impl Event {
    fn from_u32(u: sys::libclient_Event) -> Self {
        match u {
            sys::libclient_Event_LIBCLIENT_EVENT_STOPPED => Self::Stopped,
            _ => {
                panic!("Invalid libclient_Event {}", u)
            }
        }
    }
}

pub trait EventSink {
    fn on_event(&self, event: &Event);
}

struct EventSinkData {
    sink: Box<dyn EventSink>,
}

impl EventSinkData {
    pub unsafe extern "C" fn on_event_cb(
        event: *const sys::libclient_Event,
        userdata: *mut c_void,
    ) {
        let data: &EventSinkData = &*(userdata as *mut _);
        data.sink.on_event(&Event::from_u32(*event));
    }
}

fn to_cstr(s: &str) -> CString {
    CString::new(s.to_owned()).expect("Error: Unexpected null byte")
}

pub struct Config {
    ptr: *mut sys::libclient_ClientConfig,
}

impl Config {
    pub fn default(
        host: &str,
        ip: &str,
        cert: &str,
        protocol: StreamingProtocol,
        pipeline: &Pipeline,
        window_handle: &WindowHandle,
    ) -> Result<Self> {
        let chost = to_cstr(host);
        let cip = to_cstr(ip);
        let ccert = to_cstr(cert);

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_config_default(
                chost.as_ptr(),
                cip.as_ptr(),
                ccert.as_ptr(),
                protocol.to_u32(),
                pipeline.to_ptr(),
                window_handle.ptr,
                &mut ptr,
            );
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_yuv444(&self, yuv444: bool) {
        unsafe { sys::libclient_config_set_yuv444(self.ptr, yuv444) }
    }

    pub fn set_inputs(&self, inputs: bool) {
        unsafe { sys::libclient_config_set_inputs(self.ptr, inputs) }
    }

    pub fn set_audio(&self, audio: bool) {
        unsafe { sys::libclient_config_set_audio(self.ptr, audio) }
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        unsafe { sys::libclient_client_config_unref(self.ptr) }
    }
}

pub struct Client {
    ptr: *mut sys::libclient_Client,
}

impl Client {
    pub fn new(config: &Config) -> Self {
        Self {
            ptr: unsafe { sys::libclient_client_new(config.ptr) },
        }
    }

    pub fn start(&self) -> Result<()> {
        let ret = unsafe { sys::libclient_client_start(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn stop(&self) -> Result<()> {
        let ret = unsafe { sys::libclient_client_stop(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_event_handler(&self, sink: Box<dyn EventSink>) -> Result<()> {
        let handler = sys::libclient_EventHandler {
            on_event: Some(EventSinkData::on_event_cb),
            userdata: Box::into_raw(Box::new(EventSinkData { sink })) as *mut _,
        };

        let ret = unsafe { sys::libclient_client_set_event_handler(self.ptr, handler) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self {
            ptr: unsafe { sys::libclient_client_ref(self.ptr) },
        }
    }
}

extern "C" fn log_cb(
    level: sys::libclient_LogLevel,
    target: *const libc::c_char,
    line: *const libc::c_char,
    _userdata: *mut libc::c_void,
) {
    let target = unsafe { CStr::from_ptr(target).to_str().unwrap() };
    let line = unsafe { CStr::from_ptr(line).to_str().unwrap() };

    let level = match level {
        sys::libclient_LogLevel_LIBCLIENT_LOG_LEVEL_ERROR => Some(log::Level::Error),
        sys::libclient_LogLevel_LIBCLIENT_LOG_LEVEL_WARN => Some(log::Level::Warn),
        sys::libclient_LogLevel_LIBCLIENT_LOG_LEVEL_INFO => Some(log::Level::Info),
        sys::libclient_LogLevel_LIBCLIENT_LOG_LEVEL_DEBUG => Some(log::Level::Debug),
        sys::libclient_LogLevel_LIBCLIENT_LOG_LEVEL_TRACE => Some(log::Level::Trace),
        _ => None,
    };

    log::log!(target: target, level.unwrap(), "{}", line);
}

pub fn init_logger() {
    unsafe { sys::libclient_set_log_cb(Some(log_cb), std::ptr::null_mut()) };
}
