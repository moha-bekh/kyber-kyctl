use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::mem::{self, MaybeUninit};
use std::os::raw::c_void;
use std::sync::atomic::{self, AtomicPtr};

use log::{error, warn};

pub use libclient_common::metrics::Metric;
use libclient_common::metrics::{NetworkPing, NetworkStats, VideoMetric, VideoMetricData};
use libclient_sys as sys;
use libkynput_rs::Pipeline;

#[derive(Debug)]
pub enum Error {
    Generic,
}

pub type Result<T> = std::result::Result<T, Error>;

pub type Display = sys::libclient_Display;

pub type MetricsCallback = dyn FnMut(Metric) + Send + Sync;

#[cfg(feature = "kymux")]
#[derive(Clone, Copy, Debug)]
pub enum KymuxVideoProtocol {
    Reliable,
    GopStream,
    Unreliable,
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
            Self::Unreliable => {
                sys::libclient_KymuxVideoProtocol_LIBCLIENT_KYMUX_VIDEO_PROTOCOL_UNRELIABLE
            }
            Self::UnreliableFec => {
                sys::libclient_KymuxVideoProtocol_LIBCLIENT_KYMUX_VIDEO_PROTOCOL_UNRELIABLE_FEC
            }
        }
    }
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy, Debug)]
pub enum KymuxAudioProtocol {
    Reliable,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl KymuxAudioProtocol {
    fn to_u32(self) -> u32 {
        match self {
            Self::Reliable => {
                sys::libclient_KymuxAudioProtocol_LIBCLIENT_KYMUX_AUDIO_PROTOCOL_RELIABLE
            }
            Self::Unreliable => {
                sys::libclient_KymuxAudioProtocol_LIBCLIENT_KYMUX_AUDIO_PROTOCOL_UNRELIABLE
            }
            Self::UnreliableFec => {
                sys::libclient_KymuxAudioProtocol_LIBCLIENT_KYMUX_AUDIO_PROTOCOL_UNRELIABLE_FEC
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum StreamingProtocol {
    #[cfg(feature = "rtp")]
    Rtp,
    #[cfg(feature = "kymux")]
    Kymux {
        video_protocol: KymuxVideoProtocol,
        audio_protocol: KymuxAudioProtocol,
    },
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
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[derive(Clone, Debug)]
pub enum StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[derive(Clone, Debug)]
pub enum StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[derive(Clone, Debug)]
pub enum StreamerPipelineEventType {
    Stopped,
    Error,
}

#[derive(Clone, Debug)]
pub enum Event {
    ControlPlaneConnected {
        displays: Vec<Display>,
    },
    ControlPlaneConnectFailed,
    DataPlaneConnectFailed,
    DataPlaneClosed,
    Stopped {
        reason: StopReason,
    },
    DisplayListUpdated {
        displays: Vec<Display>,
    },
    InputServiceConnected,
    InputServiceDisconnected,
    StreamingStarted,
    StreamingStartFailed,
    StreamingStopped,
    StreamerPipelineEvent {
        type_: StreamerPipelineType,
        stage: StreamerPipelineStage,
        event: StreamerPipelineEventType,
    },
    AudioVolumeChanged {
        volume: f32,
    },
    AudioMuteChanged {
        mute: bool,
    },
    BitrateChanged {
        bitrate: u32,
    },
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
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_CONTROL_PLANE_CONNECTED => {
                let mut cdisplays: *mut Display = std::ptr::null_mut();
                let mut len: u32 = 0;

                let ret = unsafe {
                    sys::libclient_event_unpack_control_plane_connected(
                        event,
                        &mut cdisplays,
                        &mut len as *mut _,
                    )
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::Started");
                    return;
                }

                let displays = unsafe { std::slice::from_raw_parts(cdisplays, len as usize) };
                data.sink.on_event(Event::ControlPlaneConnected {
                    displays: displays.to_vec(),
                });
                unsafe { sys::libclient_event_free_displays(cdisplays) };
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_CONTROL_PLANE_CONNECT_FAILED => {
                data.sink.on_event(Event::ControlPlaneConnectFailed);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_DATA_PLANE_CONNECT_FAILED => {
                data.sink.on_event(Event::DataPlaneConnectFailed);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_DATA_PLANE_CLOSED => {
                data.sink.on_event(Event::DataPlaneClosed);
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
                    sys::libclient_StopReason_LIBCLIENT_STOP_REASON_CONTROL_PLANE_CLOSED => {
                        StopReason::ControlPlaneClosed
                    }
                    sys::libclient_StopReason_LIBCLIENT_STOP_REASON_CONTROL_PLANE_IO_ERROR => {
                        StopReason::ControlPlaneIoError
                    }
                    sys::libclient_StopReason_LIBCLIENT_STOP_REASON_CONTROL_PLANE_TIMEOUT => {
                        StopReason::ControlPlaneTimeout
                    }
                    sys::libclient_StopReason_LIBCLIENT_STOP_REASON_DECODE_ERROR => {
                        StopReason::DecodeError
                    }
                    _ => {
                        warn!("Fail to convert ConnectionLost {c_reason}");
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
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_INPUT_SERVICE_CONNECTED => {
                data.sink.on_event(Event::InputServiceConnected);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_INPUT_SERVICE_DISCONNECTED => {
                data.sink.on_event(Event::InputServiceDisconnected);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_STREAMING_STARTED => {
                data.sink.on_event(Event::StreamingStarted);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_STREAMING_START_FAILED => {
                data.sink.on_event(Event::StreamingStartFailed);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_STREAMING_STOPPED => {
                data.sink.on_event(Event::StreamingStopped);
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_STREAMER_PIPELINE_EVENT => {
                let mut c_type = mem::MaybeUninit::uninit();
                let mut c_stage = mem::MaybeUninit::uninit();
                let mut c_event = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::libclient_event_unpack_streamer_pipeline_event(
                        event,
                        c_type.as_mut_ptr(),
                        c_stage.as_mut_ptr(),
                        c_event.as_mut_ptr(),
                    )
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::StreamerPipelineEvent");
                    return;
                }

                let c_type = c_type.assume_init();
                let type_ = match c_type {
                    sys::libclient_StreamerPipelineType_LIBCLIENT_STREAMER_PIPELINE_TYPE_ENUMERATE_DISPLAYS => {
                        StreamerPipelineType::EnumerateDisplays
                    }
                    sys::libclient_StreamerPipelineType_LIBCLIENT_STREAMER_PIPELINE_TYPE_AUDIO => {
                        StreamerPipelineType::Audio
                    }
                    sys::libclient_StreamerPipelineType_LIBCLIENT_STREAMER_PIPELINE_TYPE_VIDEO => {
                        StreamerPipelineType::Video
                    }
                    _ => {
                        warn!("Fail to convert StreamerPipelineType {c_type}");
                        return;
                    }
                };

                let c_stage = c_stage.assume_init();
                let stage = match c_stage {
                    sys::libclient_StreamerPipelineStage_LIBCLIENT_STREAMER_PIPELINE_STAGE_PROCESS => {
                        StreamerPipelineStage::Process
                    }
                    sys::libclient_StreamerPipelineStage_LIBCLIENT_STREAMER_PIPELINE_STAGE_INITIALIZATION => {
                        StreamerPipelineStage::Initialization
                    }
                    sys::libclient_StreamerPipelineStage_LIBCLIENT_STREAMER_PIPELINE_STAGE_ACQUISITION => {
                        StreamerPipelineStage::Acquisition
                    }
                    sys::libclient_StreamerPipelineStage_LIBCLIENT_STREAMER_PIPELINE_STAGE_ENCODER => {
                        StreamerPipelineStage::Encoder
                    }
                    _ => {
                        warn!("Fail to convert StreamerPipelineStage {c_stage}");
                        return;
                    }
                };

                let c_event = c_event.assume_init();
                let event = match c_event {
                    sys::libclient_StreamerPipelineEventType_LIBCLIENT_STREAMER_PIPELINE_EVENT_TYPE_STOPPED => {
                        StreamerPipelineEventType::Stopped
                    }
                    sys::libclient_StreamerPipelineEventType_LIBCLIENT_STREAMER_PIPELINE_EVENT_TYPE_ERROR => {
                        StreamerPipelineEventType::Error
                    }
                    _ => {
                        warn!("Fail to convert StreamerPipelineEventType {c_event}");
                        return;
                    }
                };

                data.sink.on_event(Event::StreamerPipelineEvent {
                    type_,
                    stage,
                    event,
                });
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_AUDIO_VOLUME_CHANGED => {
                let mut volume = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::libclient_event_unpack_audio_volume_changed(event, volume.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::AudioVolumeChanged");
                    return;
                }

                let volume = volume.assume_init();
                data.sink.on_event(Event::AudioVolumeChanged { volume });
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_AUDIO_MUTE_CHANGED => {
                let mut mute = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::libclient_event_unpack_audio_mute_changed(event, mute.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::AudioMuteChanged");
                    return;
                }

                let mute = mute.assume_init();
                data.sink.on_event(Event::AudioMuteChanged { mute });
            }
            sys::libclient_EventType_LIBCLIENT_EVENT_TYPE_BITRATE_CHANGED => {
                let mut bitrate = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::libclient_event_unpack_bitrate_changed(event, bitrate.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::BitrateChanged");
                    return;
                }

                let bitrate = bitrate.assume_init();
                data.sink.on_event(Event::BitrateChanged { bitrate });
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

pub struct VideoPlayerConfig {
    ptr: *mut sys::libclient_VideoPlayerConfig,
}

impl VideoPlayerConfig {
    pub fn default(window_handle: Option<WindowHandle>) -> Result<Self> {
        let window_handle = if let Some(window_handle) = window_handle {
            window_handle.ptr
        } else {
            std::ptr::null_mut()
        };

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_video_player_config_default(window_handle, &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_software_decode(&self, software_decode: bool) {
        unsafe { sys::libclient_video_player_config_set_software_decode(self.ptr, software_decode) }
    }

    pub fn set_video_buffer(&self, video_buffer: u32) {
        unsafe { sys::libclient_video_player_config_set_video_buffer(self.ptr, video_buffer) }
    }
}

impl Drop for VideoPlayerConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_video_player_config_unref(self.ptr) }
    }
}

pub struct VideoSessionConfig {
    ptr: *mut sys::libclient_VideoSessionConfig,
}

impl VideoSessionConfig {
    pub fn default(display_id: u32, player: VideoPlayerConfig) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_video_session_config_default(display_id, player.ptr, &mut ptr);
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

    pub fn set_intra_refresh(&self, intra_refresh: bool) {
        unsafe { sys::libclient_video_session_config_set_intra_refresh(self.ptr, intra_refresh) }
    }
}

impl Drop for VideoSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::libclient_video_session_config_unref(self.ptr) }
    }
}

pub struct AudioSessionConfig {
    ptr: *mut sys::libclient_AudioSessionConfig,
}

impl AudioSessionConfig {
    pub fn new_default() -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_audio_session_config_default(&mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_audio_buffer(&self, audio_buffer: u32) {
        unsafe { sys::libclient_audio_session_config_set_audio_buffer(self.ptr, audio_buffer) }
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

    pub fn gamepad_force_xbox360(&self, force: bool) {
        unsafe { sys::libclient_input_session_config_gamepad_force_xbox360(self.ptr, force) };
    }

    pub fn clipboard(&self, enable: bool) {
        unsafe { sys::libclient_input_session_config_clipboard(self.ptr, enable) };
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
            StreamingProtocol::Kymux {
                video_protocol,
                audio_protocol,
            } => unsafe {
                sys::libclient_streaming_protocol_kymux(
                    video_protocol.to_u32(),
                    audio_protocol.to_u32(),
                )
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

    pub fn get_bitrate(&self) -> u32 {
        unsafe { sys::libclient_streaming_session_config_get_bitrate(self.ptr) }
    }

    pub fn set_bitrate(&self, bitrate: u32) {
        unsafe { sys::libclient_streaming_session_config_set_bitrate(self.ptr, bitrate) }
    }

    pub fn add_video(&self, config: VideoSessionConfig) {
        unsafe { sys::libclient_streaming_session_config_add_video(self.ptr, config.ptr) }

        mem::forget(config);
    }

    pub fn set_audio(&self, config: AudioSessionConfig) {
        unsafe { sys::libclient_streaming_session_config_set_audio(self.ptr, config.ptr) }
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

#[derive(Clone, Debug)]
pub enum AuthCredentials {
    Basic { username: String },
    Jwt { token: String },
}

impl AuthCredentials {
    fn as_raw(&self) -> *mut sys::libclient_AuthCredentials {
        let (type_, creds) = match self {
            AuthCredentials::Basic { username } => {
                (sys::libclient_AuthType_LIBCLIENT_AUTH_TYPE_BASIC, username)
            }
            AuthCredentials::Jwt { token } => {
                (sys::libclient_AuthType_LIBCLIENT_AUTH_TYPE_JWT, token)
            }
        };

        let creds = to_cstr(creds);
        unsafe { sys::libclient_auth_credential_new(type_, creds.as_ptr()) }
    }
}

pub struct ConnectConfig {
    ptr: *mut sys::libclient_ConnectConfig,
}

impl ConnectConfig {
    pub fn default(host: &str) -> Result<Self> {
        let chost = to_cstr(host);

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::libclient_connect_config_default(chost.as_ptr(), &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_credentials(&self, credentials: &AuthCredentials) -> Result<()> {
        let credentials = credentials.as_raw();

        let ret = unsafe { sys::libclient_connect_config_set_credentials(self.ptr, credentials) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_tls_host(&self, tls_host: &str) -> Result<()> {
        let ctlshost = to_cstr(tls_host);

        let ret =
            unsafe { sys::libclient_connect_config_set_tls_host(self.ptr, ctlshost.as_ptr()) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_tls_root_cert(&self, tls_root_cert: &str) -> Result<()> {
        let ctls_root_cert = to_cstr(tls_root_cert);

        let ret = unsafe {
            sys::libclient_connect_config_set_tls_root_cert(self.ptr, ctls_root_cert.as_ptr())
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
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
    metrics_cb: Cell<Option<Box<Box<MetricsCallback>>>>,
}

impl Client {
    pub fn new(connect_config: &ConnectConfig, metrics: bool) -> Result<Self> {
        let ptr = unsafe { sys::libclient_client_new(connect_config.ptr, metrics) };
        if !ptr.is_null() {
            Ok(Self {
                ptr,
                metrics_cb: Cell::new(None),
            })
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

    pub fn disconnect_and_stop(&self, stop_session: bool) -> Result<()> {
        let ret = unsafe { sys::libclient_client_disconnect_and_stop(self.ptr, stop_session) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn connect(&self) -> Result<()> {
        let ret = unsafe { sys::libclient_client_connect(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn disconnect(&self) -> Result<()> {
        let ret = unsafe { sys::libclient_client_disconnect(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn start_streaming(&self, session_config: &StreamingSessionConfig) -> Result<()> {
        let ret = unsafe { sys::libclient_client_start_streaming(self.ptr, session_config.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn stop_streaming(&self) -> Result<()> {
        let ret = unsafe { sys::libclient_client_stop_streaming(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let ret = unsafe { sys::libclient_client_audio_set_volume(self.ptr, volume) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let ret = unsafe { sys::libclient_client_audio_set_mute(self.ptr, mute) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_bitrate(&self, bitrate: u32) -> Result<()> {
        let ret = unsafe { sys::libclient_client_set_bitrate(self.ptr, bitrate) };
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

    pub fn set_metrics_callback(&self, cb: Option<Box<MetricsCallback>>) {
        if let Some(cb) = cb {
            unsafe {
                let mut cb = Box::new(cb);
                let userdata = &mut *cb as *mut _ as *mut c_void;
                self.metrics_cb.set(Some(cb));
                sys::libclient_client_set_metrics_callback(self.ptr, Some(metrics_cb), userdata);
            }
        } else {
            unsafe {
                sys::libclient_client_set_metrics_callback(self.ptr, None, std::ptr::null_mut());
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.metrics_cb.take().is_some() {
            self.set_metrics_callback(None);
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

pub fn init() {
    unsafe {
        sys::libclient_set_log_cb(Some(log_cb), std::ptr::null_mut());
        sys::libclient_init_crypto();
    }
}

fn to_c_metric(metric: &Metric) -> sys::libclient_Metric {
    let map_value = |v: Option<u64>| -> i64 {
        if let Some(v) = v {
            v as i64
        } else {
            -1
        }
    };

    match metric {
        Metric::Video(metric) => {
            let data = match metric.data {
                VideoMetricData::FrameAcquired => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_ACQUIRED
                }
                VideoMetricData::FrameEncoding => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODING
                }
                VideoMetricData::FrameEncoded => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODED
                }
                VideoMetricData::FrameSent => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_SENT
                }
                VideoMetricData::FrameReceived => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_RECEIVED
                }
                VideoMetricData::FrameDecoding => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_DECODING
                }
                VideoMetricData::FrameDecoded => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_DECODED
                }
                VideoMetricData::FramePrepared => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_PREPARED
                }
                VideoMetricData::FrameDisplayed => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_DISPLAYED
                }
                VideoMetricData::FrameSkipped => {
                    sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_SKIPPED
                }
            };

            sys::libclient_Metric {
                type_: sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_VIDEO,
                data: sys::libclient_MetricData {
                    video: sys::libclient_VideoMetric {
                        source_id: metric.source_id,
                        pts: metric.pts,
                        ts: metric.ts,
                        data,
                    },
                },
            }
        }
        Metric::NetworkLocal(metric) => sys::libclient_Metric {
            type_: sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_NETWORK_LOCAL,
            data: sys::libclient_MetricData {
                network_local: sys::libclient_NetworkStats {
                    rtt_micros: map_value(metric.rtt_micros),
                    packets_lost: map_value(metric.packets_lost),
                    dropped_packets: map_value(metric.dropped_packets),
                },
            },
        },
        Metric::NetworkPing(metric) => sys::libclient_Metric {
            type_: sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_NETWORK_PING,
            data: sys::libclient_MetricData {
                network_ping: sys::libclient_NetworkPing {
                    offset_micros: metric.offset_micros,
                    delay_micros: metric.delay_micros,
                },
            },
        },
        Metric::NetworkRemote(metric) => sys::libclient_Metric {
            type_: sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_NETWORK_REMOTE,
            data: sys::libclient_MetricData {
                network_local: sys::libclient_NetworkStats {
                    rtt_micros: map_value(metric.rtt_micros),
                    packets_lost: map_value(metric.packets_lost),
                    dropped_packets: map_value(metric.dropped_packets),
                },
            },
        },
    }
}

// Cannot implement From due to the orphan rule:
//  - native Rust structures are in libclient-common
//  - Rust wrappers generated from C code are un libclient-sys

fn from_metric(metric: &sys::libclient_Metric) -> Metric {
    match metric.type_ {
        sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_VIDEO => {
            let video_metric = from_video_metric(unsafe { &metric.data.video });
            Metric::Video(video_metric)
        }
        sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_NETWORK_LOCAL => {
            let network_stats = from_network_stats(unsafe { &metric.data.network_local });
            Metric::NetworkLocal(network_stats)
        }
        sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_NETWORK_PING => {
            let network_ping = from_network_ping(unsafe { &metric.data.network_ping });
            Metric::NetworkPing(network_ping)
        }
        sys::libclient_MetricType_LIBCLIENT_METRIC_TYPE_NETWORK_REMOTE => {
            let network_stats = from_network_stats(unsafe { &metric.data.network_remote });
            Metric::NetworkRemote(network_stats)
        }
        v => panic!("Unexpected metric type: {v}"),
    }
}

fn from_video_metric(video_metric: &sys::libclient_VideoMetric) -> VideoMetric {
    VideoMetric {
        source_id: video_metric.source_id,
        pts: video_metric.pts,
        ts: video_metric.ts,
        data: from_video_metric_data(&video_metric.data),
    }
}

fn from_video_metric_data(video_metric_data: &sys::libclient_VideoMetricData) -> VideoMetricData {
    match *video_metric_data {
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_ACQUIRED => {
            VideoMetricData::FrameAcquired
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODING => {
            VideoMetricData::FrameEncoding
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODED => {
            VideoMetricData::FrameEncoded
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_SENT => {
            VideoMetricData::FrameSent
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_RECEIVED => {
            VideoMetricData::FrameReceived
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_DECODING => {
            VideoMetricData::FrameDecoding
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_DECODED => {
            VideoMetricData::FrameDecoded
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_PREPARED => {
            VideoMetricData::FramePrepared
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_DISPLAYED => {
            VideoMetricData::FrameDisplayed
        }
        sys::libclient_VideoMetricData_LIBCLIENT_VIDEO_METRIC_DATA_FRAME_SKIPPED => {
            VideoMetricData::FrameSkipped
        }
        v => panic!("Unexpected video metric data type: {v}"),
    }
}

fn from_network_stats(network_stats: &sys::libclient_NetworkStats) -> NetworkStats {
    let map_value = |v: i64| -> Option<u64> {
        if v >= 0 {
            Some(v as u64)
        } else {
            None
        }
    };

    NetworkStats {
        rtt_micros: map_value(network_stats.rtt_micros),
        packets_lost: map_value(network_stats.packets_lost),
        dropped_packets: map_value(network_stats.dropped_packets),
    }
}

fn from_network_ping(network_ping: &sys::libclient_NetworkPing) -> NetworkPing {
    NetworkPing {
        offset_micros: network_ping.offset_micros,
        delay_micros: network_ping.delay_micros,
    }
}

extern "C" fn metrics_cb(metric: *const sys::libclient_Metric, userdata: *mut c_void) {
    let cb: &mut MetricsCallback = unsafe { &mut **(userdata as *mut Box<MetricsCallback>) };
    let metric = from_metric(unsafe { &*metric });
    cb(metric);
}

pub struct MetricsJSONProducer {
    ptr: AtomicPtr<sys::libclient_metrics_json_producer>,
}

impl Default for MetricsJSONProducer {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsJSONProducer {
    pub fn new() -> Self {
        let ptr = unsafe { sys::libclient_metrics_json_producer_new() };
        Self {
            ptr: AtomicPtr::new(ptr),
        }
    }

    pub fn push(&self, metric: &Metric) -> Vec<Vec<u8>> {
        let ptr = self.ptr.load(atomic::Ordering::Relaxed);
        let metric = to_c_metric(metric);

        let c_json_list =
            unsafe { sys::libclient_metrics_json_producer_push(ptr, &metric as *const _) };

        let c_values = unsafe {
            let mut c_values = MaybeUninit::uninit();
            let mut c_values_count = MaybeUninit::uninit();

            sys::libclient_metrics_json_values(
                c_json_list,
                c_values.as_mut_ptr(),
                c_values_count.as_mut_ptr(),
            );

            let c_values = c_values.assume_init();
            let c_values_count = c_values_count.assume_init();

            std::slice::from_raw_parts(c_values, c_values_count)
        };

        let values: Vec<_> = c_values
            .iter()
            .map(|v| {
                let s = unsafe { CStr::from_ptr(*v) };
                s.to_bytes().to_vec()
            })
            .collect();

        unsafe {
            sys::libclient_metrics_json_unref(c_json_list);
        }

        values
    }
}

impl Drop for MetricsJSONProducer {
    fn drop(&mut self) {
        let ptr = self.ptr.load(atomic::Ordering::Relaxed);
        unsafe { sys::libclient_metrics_json_producer_unref(ptr) };
    }
}
