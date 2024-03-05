use std::sync::Arc;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use log::warn;

use libkynput_rs::Pipeline;

use crate::VideoCodec;

#[allow(non_camel_case_types)]
pub type Display = super::Display;

#[allow(non_camel_case_types)]
pub struct libclient_WindowHandle {
    handle: crate::platform::WindowHandle,
}

#[allow(non_camel_case_types)]
pub struct libclient_ConnectConfig {
    config: crate::ConnectConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_StreamingProtocol {
    protocol: crate::StreamingProtocol,
}

#[allow(non_camel_case_types)]
pub struct libclient_StreamingSessionConfig {
    config: crate::StreamingSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_PlayerConfig {
    config: crate::PlayerConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_VideoSessionConfig {
    config: crate::VideoSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_InputSessionConfig {
    config: crate::InputSessionConfig,
}

fn result_to_c_int<T>(result: crate::Result<T>) -> c_int {
    match result {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum libclient_KymuxVideoProtocol {
    Reliable,
    GopStream,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<libclient_KymuxVideoProtocol> for kymux::VideoProtocol {
    fn from(value: libclient_KymuxVideoProtocol) -> Self {
        match value {
            libclient_KymuxVideoProtocol::Reliable => Self::Reliable,
            libclient_KymuxVideoProtocol::GopStream => Self::GopStream,
            libclient_KymuxVideoProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

#[repr(C)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<log::Level> for LogLevel {
    fn from(t: log::Level) -> Self {
        match t {
            log::Level::Error => Self::Error,
            log::Level::Warn => Self::Warn,
            log::Level::Info => Self::Info,
            log::Level::Debug => Self::Debug,
            log::Level::Trace => Self::Trace,
        }
    }
}

type LogCb = extern "C" fn(
    level: LogLevel,
    target: *const c_char,
    line: *const c_char,
    userdata: *mut c_void,
);

struct Logger {
    cb: LogCb,
    userdata: std::sync::atomic::AtomicPtr<c_void>,
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let line = if let Ok(line) = CString::new(format!("{}", record.args())) {
            line
        } else {
            return;
        };

        let target = if let Ok(target) = CString::new(record.target()) {
            target
        } else {
            return;
        };

        (self.cb)(
            record.level().into(),
            target.as_ptr(),
            line.as_ptr(),
            self.userdata.load(std::sync::atomic::Ordering::Relaxed),
        );
    }

    fn flush(&self) {}
}

#[no_mangle]
pub unsafe extern "C" fn libclient_set_log_cb(cb: LogCb, userdata: *mut c_void) {
    let userdata = std::sync::atomic::AtomicPtr::new(userdata);
    let logger = Box::new(Logger { cb, userdata });

    if log::set_boxed_logger(logger).is_err() {
        return;
    }

    log::set_max_level(log::LevelFilter::Trace);
}

#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_x11(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::Xlib(handle),
    }))
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_ns_view(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::AppKit(handle),
    }))
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_hwnd(
    hwnd: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::Win32(hwnd),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_default(
    host: *const c_char,
    tls_host: *const c_char,
    tls_root_cert: *const c_char,
    out_config: *mut *mut libclient_ConnectConfig,
) -> c_int {
    let Ok(host) = CStr::from_ptr(host).to_str() else {
        warn!("Fail to convert client Host");
        return -1;
    };

    let Ok(tls_host) = CStr::from_ptr(tls_host).to_str() else {
        warn!("Fail to convert client TLS host");
        return -1;
    };

    let Ok(tls_root_cert) = CStr::from_ptr(tls_root_cert).to_str() else {
        warn!("Fail to convert Root TLS certificate");
        return -1;
    };

    let config = crate::ConnectConfig {
        host: host.into(),
        port: crate::STREAMER_PORT,
        tls_host: tls_host.into(),
        tls_root_cert: tls_root_cert.into(),
    };

    *out_config = Box::into_raw(Box::new(libclient_ConnectConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_streamer_port(
    config: *mut libclient_ConnectConfig,
    port: u16,
) {
    (*config).config.port = port;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_unref(cfg: *mut libclient_ConnectConfig) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[cfg(feature = "rtp")]
#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_protocol_rtp() -> *mut libclient_StreamingProtocol {
    let protocol = crate::StreamingProtocol::Rtp;

    Box::into_raw(Box::new(libclient_StreamingProtocol { protocol }))
}

#[cfg(feature = "kymux")]
#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_protocol_kymux(
    video_protocol: libclient_KymuxVideoProtocol,
) -> *mut libclient_StreamingProtocol {
    let protocol = crate::StreamingProtocol::Kymux(crate::kymux_backend::KymuxConfig {
        video_protocol: Some(video_protocol.into()),
        backend: crate::kymux_backend::KymuxTransportBackend::Quic,
    });

    Box::into_raw(Box::new(libclient_StreamingProtocol { protocol }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_protocol_unref(
    protocol: *mut libclient_StreamingProtocol,
) {
    if !protocol.is_null() {
        let _ = Box::from_raw(protocol);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_player_config_unref(cfg: *mut libclient_PlayerConfig) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_player_config_default(
    window_handle: *mut libclient_WindowHandle,
    out_config: *mut *mut libclient_PlayerConfig,
) -> c_int {
    let window_handle = if !window_handle.is_null() {
        let window_handle = Box::from_raw(window_handle);
        Some(window_handle.handle)
    } else {
        None
    };

    let config = crate::PlayerConfig {
        window_handle,
        software_decode: false,
    };

    *out_config = Box::into_raw(Box::new(libclient_PlayerConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_default(
    display_index: usize,
    out_config: *mut *mut libclient_VideoSessionConfig,
) -> c_int {
    let config = crate::VideoSessionConfig {
        display_index,
        codec: VideoCodec::H264,
        yuv444: false,
    };

    *out_config = Box::into_raw(Box::new(libclient_VideoSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_player_config_set_software_decode(
    config: *mut libclient_PlayerConfig,
    software_decode: bool,
) {
    (*config).config.software_decode = software_decode;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_unref(
    cfg: *mut libclient_VideoSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_set_yuv444(
    config: *mut libclient_VideoSessionConfig,
    yuv444: bool,
) {
    (*config).config.yuv444 = yuv444;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_set_codec(
    config: *mut libclient_VideoSessionConfig,
    codec: VideoCodec,
) {
    (*config).config.codec = codec;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_input_session_config_default(
    pipeline: *mut libkynput_sys::libkynput_Pipeline,
    out_config: *mut *mut libclient_InputSessionConfig,
) -> c_int {
    let pipeline = Pipeline::from_ptr(pipeline);

    let config = crate::InputSessionConfig { pipeline };

    *out_config = Box::into_raw(Box::new(libclient_InputSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_input_session_config_unref(
    cfg: *mut libclient_InputSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_default(
    protocol: *const libclient_StreamingProtocol,
    out_config: *mut *mut libclient_StreamingSessionConfig,
) -> c_int {
    let config = crate::StreamingSessionConfig {
        protocol: (*protocol).protocol.clone(),
        bitrate: 20_000_000,
        video: None,
        audio: false,
        input: None,
    };

    *out_config = Box::into_raw(Box::new(libclient_StreamingSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_unref(
    cfg: *mut libclient_StreamingSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_bitrate(
    config: *mut libclient_StreamingSessionConfig,
    bitrate: u32,
) {
    (*config).config.bitrate = bitrate;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_audio(
    config: *mut libclient_StreamingSessionConfig,
    audio: bool,
) {
    (*config).config.audio = audio;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_video(
    config: *mut libclient_StreamingSessionConfig,
    video: *mut libclient_VideoSessionConfig,
) {
    let video = Box::from_raw(video);

    (*config).config.video = Some(video.config);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_input(
    config: *mut libclient_StreamingSessionConfig,
    input: *mut libclient_InputSessionConfig,
) {
    let input = Box::from_raw(input);

    (*config).config.input = Some(input.config);
}

#[allow(non_camel_case_types)]
pub struct libclient_Client {
    client: Arc<crate::Client>,
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_new(
    connect_config: *mut libclient_ConnectConfig,
) -> *mut libclient_Client {
    let connect_config = (*connect_config).config.clone();

    let client = crate::Client::new(connect_config);
    if let Ok(client) = client {
        Box::into_raw(Box::new(libclient_Client {
            client: Arc::new(client),
        }))
    } else {
        ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_stop(client: *mut libclient_Client) -> c_int {
    let ret = (*client).client.stop();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_connect(client: *mut libclient_Client) -> c_int {
    let ret = (*client).client.connect();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_disconnect(client: *mut libclient_Client) -> c_int {
    let ret = (*client).client.disconnect();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_start_streaming(
    client: *mut libclient_Client,
    session_config: *mut libclient_StreamingSessionConfig,
    player_config: *mut libclient_PlayerConfig,
) -> c_int {
    let session_config = (*session_config).config.clone();
    let player_config = (*player_config).config.clone();

    let ret = (*client)
        .client
        .start_streaming(session_config, player_config);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_stop_streaming(client: *mut libclient_Client) -> c_int {
    let ret = (*client).client.stop_streaming();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_ref(
    client: *mut libclient_Client,
) -> *mut libclient_Client {
    Box::into_raw(Box::new(libclient_Client {
        client: (*client).client.clone(),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_unref(client: *mut libclient_Client) {
    let _ = Box::from_raw(client);
}

#[allow(non_camel_case_types)]
pub type Event = super::Event;

#[repr(C)]
pub enum libclient_EventType {
    ControlPlaneConnected,
    ControlPlaneConnectFailed,
    DataPlaneClosed,
    Stopped,
    DisplayListUpdated,
    StreamingStarted,
    StreamingStopped,
    StreamerPipelineEvent,
}

#[repr(C)]
pub enum libclient_StopReason {
    Requested,
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[repr(C)]
pub enum libclient_StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[repr(C)]
pub enum libclient_StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[repr(C)]
pub enum libclient_StreamerPipelineEventType {
    Stopped,
    Error,
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_get_type(event: *const Event) -> libclient_EventType {
    match *event {
        Event::ControlPlaneConnected { .. } => libclient_EventType::ControlPlaneConnected,
        Event::ControlPlaneConnectFailed => libclient_EventType::ControlPlaneConnectFailed,
        Event::DataPlaneClosed => libclient_EventType::DataPlaneClosed,
        Event::Stopped { .. } => libclient_EventType::Stopped,
        Event::DisplayListUpdated { .. } => libclient_EventType::DisplayListUpdated,
        Event::StreamingStarted => libclient_EventType::StreamingStarted,
        Event::StreamingStopped => libclient_EventType::StreamingStopped,
        Event::StreamerPipelineEvent { .. } => libclient_EventType::StreamerPipelineEvent,
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_control_plane_connected(
    event: *const Event,
    out_displays: *mut *mut Display,
    out_len: *mut u32,
) -> c_int {
    if let Event::ControlPlaneConnected { displays } = &*event {
        let mut displays = Box::new(displays.clone());

        *out_displays = displays.as_mut_ptr();
        *out_len = displays.len() as u32;

        std::mem::forget(displays);

        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_stopped(
    event: *const Event,
    out_reason: *mut libclient_StopReason,
) -> c_int {
    let Event::Stopped { reason } = &*event else {
        return -1;
    };

    *out_reason = match reason {
        crate::StopReason::Requested => libclient_StopReason::Requested,
        crate::StopReason::ControlPlaneClosed => libclient_StopReason::ControlPlaneClosed,
        crate::StopReason::ControlPlaneIoError => libclient_StopReason::ControlPlaneIoError,
        crate::StopReason::ControlPlaneTimeout => libclient_StopReason::ControlPlaneTimeout,
        crate::StopReason::DecodeError => libclient_StopReason::DecodeError,
    };

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_display_list_updated(
    event: *const Event,
    out_displays: *mut *mut Display,
    out_len: *mut u32,
) -> c_int {
    if let Event::DisplayListUpdated { displays } = &*event {
        let mut displays = Box::new(displays.clone());

        *out_displays = displays.as_mut_ptr();
        *out_len = displays.len() as u32;

        std::mem::forget(displays);

        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_free_displays(displays: *mut Display) {
    let _ = Box::from_raw(displays);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_streamer_pipeline_event(
    event: *const Event,
    out_type: *mut libclient_StreamerPipelineType,
    out_stage: *mut libclient_StreamerPipelineStage,
    out_event: *mut libclient_StreamerPipelineEventType,
) -> c_int {
    let Event::StreamerPipelineEvent {
        type_,
        stage,
        event,
    } = &*event
    else {
        return -1;
    };

    if out_type.is_null() || out_stage.is_null() || out_event.is_null() {
        return -1;
    }

    *out_type = match type_ {
        crate::StreamerPipelineType::EnumerateDisplays => {
            libclient_StreamerPipelineType::EnumerateDisplays
        }
        crate::StreamerPipelineType::Audio => libclient_StreamerPipelineType::Audio,
        crate::StreamerPipelineType::Video => libclient_StreamerPipelineType::Video,
    };

    *out_stage = match stage {
        crate::StreamerPipelineStage::Process => libclient_StreamerPipelineStage::Process,
        crate::StreamerPipelineStage::Initialization => {
            libclient_StreamerPipelineStage::Initialization
        }
        crate::StreamerPipelineStage::Acquisition => libclient_StreamerPipelineStage::Acquisition,
        crate::StreamerPipelineStage::Encoder => libclient_StreamerPipelineStage::Encoder,
    };

    *out_event = match event {
        crate::StreamerPipelineEventType::Stopped => libclient_StreamerPipelineEventType::Stopped,
        crate::StreamerPipelineEventType::Error => libclient_StreamerPipelineEventType::Error,
    };

    0
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct libclient_EventHandler {
    on_event: Option<extern "C" fn(event: *const Event, userdata: *mut c_void)>,
    userdata: *mut c_void,
}

unsafe impl Send for libclient_EventHandler {}
unsafe impl Sync for libclient_EventHandler {}

impl crate::event_manager::EventSink for libclient_EventHandler {
    fn on_event(&self, event: &crate::Event) {
        if let Some(on_event) = self.on_event {
            on_event(event, self.userdata);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_set_event_handler(
    client: *mut libclient_Client,
    handler: libclient_EventHandler,
) -> c_int {
    (*client)
        .client
        .event_manager
        .add_event_sink(Box::new(handler));

    0
}
