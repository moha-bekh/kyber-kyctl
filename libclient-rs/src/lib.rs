use std::ffi::{CStr, CString};
use std::mem;
use std::os::raw::c_void;

use log::{error, warn};

use libclient_sys as sys;
use libkynput_rs::Pipeline;

#[derive(Debug)]
pub enum Error {
    Generic,
}

pub type Result<T> = std::result::Result<T, Error>;

pub type Display = sys::libclient_Display;

#[cfg(feature = "kymux")]
#[derive(Clone, Copy, Debug)]
pub enum KymuxVideoProtocol {
    Reliable,
    GopStream,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl KymuxVideoProtocol {
    fn to_u32(self) -> u32 {
        match self {
            Self::Reliable => {
                sys::libclient_KymuxVideoProtocol_LIBCLIENT_KYMUX_VIDEO_PROTOCOL_RELIABLE
            }
            Self::GopStream => {
                sys::libclient_KymuxVideoProtocol_LIBCLIENT_KYMUX_VIDEO_PROTOCOL_GOP_STREAM
            }
            Self::UnreliableFec => {
                sys::libclient_KymuxVideoProtocol_LIBCLIENT_KYMUX_VIDEO_PROTOCOL_UNRELIABLE_FEC
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum StreamingProtocol {
    #[cfg(feature = "rtp")]
    Rtp,
    #[cfg(feature = "kymux")]
    Kymux(KymuxVideoProtocol),
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

#[derive(Clone, Debug)]
pub enum StopReason {
    Requested,
}

#[derive(Clone, Debug)]
pub enum Event {
    Started { displays: Vec<Display> },
    Stopped { reason: StopReason },
    DisplayListUpdated { displays: Vec<Display> },
}

pub trait EventSink {
    fn on_event(&self, event: Event);
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

        let event_type = unsafe { sys::libclient_event_get_type(event) };
        match event_type {
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_STARTED => {
                let mut cdisplays: *mut Display = std::ptr::null_mut();
                let mut len: u32 = 0;

                let ret = unsafe {
                    sys::libclient_event_unpack_started(event, &mut cdisplays, &mut len as *mut _)
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::Started");
                    return;
                }

                let displays = unsafe { std::slice::from_raw_parts(cdisplays, len as usize) };
                data.sink.on_event(Event::Started {
                    displays: displays.to_vec(),
                });
                unsafe { sys::libclient_event_free_displays(cdisplays) };
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_STOPPED => {
                let mut c_reason = mem::MaybeUninit::uninit();

                let ret =
                    unsafe { sys::libclient_event_unpack_stopped(event, c_reason.as_mut_ptr()) };
                if ret != 0 {
                    warn!("Fail to unpack EventType::Stopped");
                    return;
                }

                let c_reason = c_reason.assume_init();
                let reason = match c_reason {
                    sys::libclient_StopReason_LIBCLIENT_STOP_REASON_REQUESTED => {
                        StopReason::Requested
                    }
                    _ => {
                        warn!("Fail to convert StopReason {c_reason}");
                        return;
                    }
                };

                data.sink.on_event(Event::Stopped { reason });
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_DISPLAY_LIST_UPDATED => {
                let mut cdisplays: *mut Display = std::ptr::null_mut();
                let mut len: u32 = 0;

                let ret = unsafe {
                    sys::libclient_event_unpack_display_list_updated(
                        event,
                        &mut cdisplays,
                        &mut len as *mut _,
                    )
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::DisplayListUpdated");
                    return;
                }

                let displays = unsafe { std::slice::from_raw_parts(cdisplays, len as usize) };
                data.sink.on_event(Event::DisplayListUpdated {
                    displays: displays.to_vec(),
                });
                unsafe { sys::libclient_event_free_displays(cdisplays) };
            }
            _ => {
                error!("Invalid libclient_EventType {}", event_type)
            }
        }
    }
}

fn to_cstr(s: &str) -> CString {
    CString::new(s.to_owned()).expect("Error: Unexpected null byte")
}

pub struct PlayerConfig {
    ptr: *mut sys::libclient_PlayerConfig,
}

impl PlayerConfig {
    pub fn default(window_handle: Option<WindowHandle>) -> Result<Self> {
        let window_handle = if let Some(window_handle) = window_handle {
            window_handle.ptr
        } else {
            std::ptr::null_mut()
        };

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_player_config_default(window_handle, &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_software_decode(&self, software_decode: bool) {
        unsafe { sys::libclient_player_config_set_software_decode(self.ptr, software_decode) }
    }
}

impl Drop for PlayerConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_player_config_unref(self.ptr) }
    }
}

pub struct VideoSessionConfig {
    ptr: *mut sys::libclient_VideoSessionConfig,
}

impl VideoSessionConfig {
    pub fn default(display_index: usize) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_video_session_config_default(display_index, &mut ptr);
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
        let streaming_protocol = match protocol {
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => unsafe { sys::libclient_streaming_protocol_rtp() },
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux(video_protocol) => unsafe {
                sys::libclient_streaming_protocol_kymux(video_protocol.to_u32())
            },
        };

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_streaming_session_config_default(streaming_protocol, &mut ptr);
            sys::libclient_streaming_protocol_unref(streaming_protocol);

            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_bitrate(&self, bitrate: u32) {
        unsafe { sys::libclient_streaming_session_config_set_bitrate(self.ptr, bitrate) }
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
    pub fn default(host: &str, tls_host: &str, tls_root_cert: &str) -> Result<Self> {
        let chost = to_cstr(host);
        let ctlshost = to_cstr(tls_host);
        let ctls_root_cert = to_cstr(tls_root_cert);

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_connect_config_default(
                chost.as_ptr(),
                ctlshost.as_ptr(),
                ctls_root_cert.as_ptr(),
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
        player_config: &PlayerConfig,
    ) -> Result<Self> {
        let ptr = unsafe {
            sys::libclient_client_new(connect_config.ptr, session_config.ptr, player_config.ptr)
        };
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
