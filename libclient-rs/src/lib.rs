use std::ffi::{CStr, CString};
use std::mem;
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

#[derive(Clone, Copy, Debug)]
pub enum VideoCodec {
    H264,
    HEVC,
    AV1,
}

impl VideoCodec {
    pub fn from_u32(u: sys::libclient_VideoCodec) -> Option<Self> {
        match u {
            sys::libclient_VideoCodec_LIBCLIENT_VIDEO_CODEC_H264 => Some(Self::H264),
            sys::libclient_VideoCodec_LIBCLIENT_VIDEO_CODEC_HEVC => Some(Self::HEVC),
            sys::libclient_VideoCodec_LIBCLIENT_VIDEO_CODEC_AV1 => Some(Self::AV1),
            _ => None,
        }
    }

    fn to_u32(self) -> u32 {
        match self {
            Self::H264 => sys::libclient_VideoCodec_LIBCLIENT_VIDEO_CODEC_H264,
            Self::HEVC => sys::libclient_VideoCodec_LIBCLIENT_VIDEO_CODEC_HEVC,
            Self::AV1 => sys::libclient_VideoCodec_LIBCLIENT_VIDEO_CODEC_AV1,
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

pub struct VideoSessionConfig {
    ptr: *mut sys::libclient_VideoSessionConfig,
}

impl VideoSessionConfig {
    pub fn default(window_handle: &WindowHandle) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_video_session_config_default(window_handle.ptr, &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_codec(&self, codec: VideoCodec) {
        unsafe { sys::libclient_video_session_config_set_codec(self.ptr, codec.to_u32()) }
    }

    pub fn set_yuv444(&self, yuv444: bool) {
        unsafe { sys::libclient_video_session_config_set_yuv444(self.ptr, yuv444) }
    }
}

impl Drop for VideoSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_video_session_config_unref(self.ptr) }
    }
}

pub struct InputSessionConfig {
    ptr: *mut sys::libclient_InputSessionConfig,
}

impl InputSessionConfig {
    pub fn default(pipeline: &Pipeline) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_input_session_config_default(pipeline.to_ptr(), &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }
}

impl Drop for InputSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_input_session_config_unref(self.ptr) }
    }
}

pub struct StreamingSessionConfig {
    ptr: *mut sys::libclient_StreamingSessionConfig,
}

impl StreamingSessionConfig {
    pub fn default(protocol: StreamingProtocol) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_streaming_session_config_default(protocol.to_u32(), &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_video(&self, config: VideoSessionConfig) {
        unsafe { sys::libclient_streaming_session_config_set_video(self.ptr, config.ptr) }

        mem::forget(config);
    }

    pub fn set_audio(&self, enabled: bool) {
        unsafe { sys::libclient_streaming_session_config_set_audio(self.ptr, enabled) }
    }

    pub fn set_input(&self, config: InputSessionConfig) {
        unsafe { sys::libclient_streaming_session_config_set_input(self.ptr, config.ptr) }

        mem::forget(config);
    }
}

impl Drop for StreamingSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_streaming_session_config_unref(self.ptr) }
    }
}

pub struct ConnectConfig {
    ptr: *mut sys::libclient_ConnectConfig,
}

impl ConnectConfig {
    pub fn default(ip: &str, tls_host: &str, tls_cert: &str) -> Result<Self> {
        let ctlshost = to_cstr(tls_host);
        let cip = to_cstr(ip);
        let ctls_cert = to_cstr(tls_cert);

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_connect_config_default(
                cip.as_ptr(),
                ctlshost.as_ptr(),
                ctls_cert.as_ptr(),
                &mut ptr,
            );
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_streamer_port(&self, port: u16) {
        unsafe { sys::libclient_connect_config_set_streamer_port(self.ptr, port) }
    }
}

impl Drop for ConnectConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_connect_config_unref(self.ptr) }
    }
}

pub struct Client {
    ptr: *mut sys::libclient_Client,
}

impl Client {
    pub fn new(
        connect_config: &ConnectConfig,
        session_config: &StreamingSessionConfig,
    ) -> Result<Self> {
        let ptr = unsafe { sys::libclient_client_new(connect_config.ptr, session_config.ptr) };
        if !ptr.is_null() {
            Ok(Self { ptr })
        } else {
            Err(Error::Generic)
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
