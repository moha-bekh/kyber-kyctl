// Project Kyber: lib.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::cell::Cell;
use std::ffi::{CStr, CString};
use std::mem::{self, MaybeUninit};
use std::os::raw::{c_char, c_void};
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicPtr};

use log::{error, warn};

pub use kyclient_common::{auth::AuthCredentials, metrics::Metric};

/// TOFU (Trust-On-First-Use) prompt for certificate verification decisions.
#[derive(Clone, Debug)]
pub enum TofuPrompt {
    /// A new host was encountered that is not in known_hosts.
    UnknownHost {
        hostname: String,
        fingerprint: String,
    },
    /// A known host presented a different certificate than expected.
    CertificateChanged {
        hostname: String,
        expected_fingerprint: String,
        actual_fingerprint: String,
    },
}
use kyclient_common::metrics::{NetworkPing, NetworkStats, VideoMetric, VideoMetricData};
use kyclient_sys as sys;
use kynput_rs::Pipeline;

#[derive(Debug)]
pub enum Error {
    Generic,
}

pub type Result<T> = std::result::Result<T, Error>;

pub type Display = sys::kyclient_Display;

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
                sys::kyclient_KymuxVideoProtocol_KYCLIENT_KYMUX_VIDEO_PROTOCOL_RELIABLE
            }
            Self::GopStream => {
                sys::kyclient_KymuxVideoProtocol_KYCLIENT_KYMUX_VIDEO_PROTOCOL_GOP_STREAM
            }
            Self::Unreliable => {
                sys::kyclient_KymuxVideoProtocol_KYCLIENT_KYMUX_VIDEO_PROTOCOL_UNRELIABLE
            }
            Self::UnreliableFec => {
                sys::kyclient_KymuxVideoProtocol_KYCLIENT_KYMUX_VIDEO_PROTOCOL_UNRELIABLE_FEC
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
                sys::kyclient_KymuxAudioProtocol_KYCLIENT_KYMUX_AUDIO_PROTOCOL_RELIABLE
            }
            Self::Unreliable => {
                sys::kyclient_KymuxAudioProtocol_KYCLIENT_KYMUX_AUDIO_PROTOCOL_UNRELIABLE
            }
            Self::UnreliableFec => {
                sys::kyclient_KymuxAudioProtocol_KYCLIENT_KYMUX_AUDIO_PROTOCOL_UNRELIABLE_FEC
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
    pub fn from_u32(u: sys::kyclient_VideoCodec) -> Option<Self> {
        match u {
            sys::kyclient_VideoCodec_KYCLIENT_VIDEO_CODEC_H264 => Some(Self::H264),
            sys::kyclient_VideoCodec_KYCLIENT_VIDEO_CODEC_HEVC => Some(Self::HEVC),
            sys::kyclient_VideoCodec_KYCLIENT_VIDEO_CODEC_AV1 => Some(Self::AV1),
            _ => None,
        }
    }

    fn to_u32(self) -> u32 {
        match self {
            Self::H264 => sys::kyclient_VideoCodec_KYCLIENT_VIDEO_CODEC_H264,
            Self::HEVC => sys::kyclient_VideoCodec_KYCLIENT_VIDEO_CODEC_HEVC,
            Self::AV1 => sys::kyclient_VideoCodec_KYCLIENT_VIDEO_CODEC_AV1,
        }
    }
}

pub struct WindowHandle {
    pub ptr: *mut sys::kyclient_WindowHandle,
}

impl WindowHandle {
    #[cfg(target_os = "linux")]
    pub fn from_x11(handle: u64) -> Self {
        Self {
            ptr: unsafe { sys::kyclient_windowshandle_from_x11(handle) },
        }
    }

    #[cfg(target_os = "macos")]
    pub fn from_ns_view(handle: u64) -> Self {
        Self {
            ptr: unsafe { sys::kyclient_windowshandle_from_ns_view(handle) },
        }
    }

    #[cfg(target_os = "windows")]
    pub fn from_hwnd(hwnd: u64) -> Self {
        Self {
            ptr: unsafe { sys::kyclient_windowshandle_from_hwnd(hwnd) },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectFailedReason {
    NetworkError,
    Timeout,
    Unauthorized,
    ServerError,
    TlsVerification,
    TlsConfiguration,
    WebSocket,
    Protocol,
}

#[derive(Clone, Debug, PartialEq)]
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
    ControlPlaneConnectFailed {
        reason: ConnectFailedReason,
    },
    AuthenticationRequired {
        issuer: String,
        client_id: String,
    },
    DataPlaneConnectFailed,
    DataPlaneClosed,
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
    },
    Reconnected {
        should_restart_streaming: bool,
    },
    ReconnectionFailed,
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
    pub unsafe extern "C" fn on_event_cb(event: *const sys::kyclient_Event, userdata: *mut c_void) {
        let data: &EventSinkData = &*(userdata as *mut _);

        let event_type = unsafe { sys::kyclient_event_get_type(event) };
        match event_type {
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_CONTROL_PLANE_CONNECTED => {
                let mut cdisplays: *mut Display = std::ptr::null_mut();
                let mut len: u32 = 0;

                let ret = unsafe {
                    sys::kyclient_event_unpack_control_plane_connected(
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
                unsafe { sys::kyclient_event_free_displays(cdisplays) };
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_CONTROL_PLANE_CONNECT_FAILED => {
                let mut c_reason = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::kyclient_event_unpack_connect_failed(event, c_reason.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::ControlPlaneConnectFailed");
                    return;
                }

                let c_reason = c_reason.assume_init();
                let reason = match c_reason {
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_NETWORK_ERROR => {
                        ConnectFailedReason::NetworkError
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_TIMEOUT => {
                        ConnectFailedReason::Timeout
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_UNAUTHORIZED => {
                        ConnectFailedReason::Unauthorized
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_SERVER_ERROR => {
                        ConnectFailedReason::ServerError
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_TLS_VERIFICATION => {
                        ConnectFailedReason::TlsVerification
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_TLS_CONFIGURATION => {
                        ConnectFailedReason::TlsConfiguration
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_WEB_SOCKET => {
                        ConnectFailedReason::WebSocket
                    }
                    sys::kyclient_ConnectFailedReason_KYCLIENT_CONNECT_FAILED_REASON_PROTOCOL => {
                        ConnectFailedReason::Protocol
                    }
                    _ => {
                        warn!("Fail to convert ConnectFailedReason {c_reason}");
                        return;
                    }
                };

                data.sink
                    .on_event(Event::ControlPlaneConnectFailed { reason });
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_AUTHENTICATION_REQUIRED => {
                let mut c_issuer: *mut c_char = std::ptr::null_mut();
                let mut c_client_id: *mut c_char = std::ptr::null_mut();

                let ret = unsafe {
                    sys::kyclient_event_unpack_authentication_required(
                        event,
                        &mut c_issuer,
                        &mut c_client_id,
                    )
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::AuthenticationRequired");
                    return;
                }

                let issuer = unsafe { CStr::from_ptr(c_issuer) }
                    .to_string_lossy()
                    .into_owned();
                let client_id = unsafe { CStr::from_ptr(c_client_id) }
                    .to_string_lossy()
                    .into_owned();

                data.sink
                    .on_event(Event::AuthenticationRequired { issuer, client_id });

                unsafe { sys::kyclient_event_free_authentication_required(c_issuer, c_client_id) };
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_DATA_PLANE_CONNECT_FAILED => {
                data.sink.on_event(Event::DataPlaneConnectFailed);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_DATA_PLANE_CLOSED => {
                data.sink.on_event(Event::DataPlaneClosed);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_RECONNECTING => {
                let mut attempt: u32 = 0;
                let mut max_attempts: u32 = 0;

                let ret = unsafe {
                    sys::kyclient_event_unpack_reconnecting(event, &mut attempt, &mut max_attempts)
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::Reconnecting");
                    return;
                }

                data.sink.on_event(Event::Reconnecting {
                    attempt,
                    max_attempts,
                });
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_RECONNECTED => {
                let mut should_restart_streaming: bool = false;

                let ret = unsafe {
                    sys::kyclient_event_unpack_reconnected(event, &mut should_restart_streaming)
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::Reconnected");
                    return;
                }

                data.sink.on_event(Event::Reconnected {
                    should_restart_streaming,
                });
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_RECONNECTION_FAILED => {
                data.sink.on_event(Event::ReconnectionFailed);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_STOPPED => {
                let mut c_reason = mem::MaybeUninit::uninit();

                let ret =
                    unsafe { sys::kyclient_event_unpack_stopped(event, c_reason.as_mut_ptr()) };
                if ret != 0 {
                    warn!("Fail to unpack EventType::Stopped");
                    return;
                }

                let c_reason = c_reason.assume_init();
                let reason = match c_reason {
                    sys::kyclient_StopReason_KYCLIENT_STOP_REASON_REQUESTED => {
                        StopReason::Requested
                    }
                    sys::kyclient_StopReason_KYCLIENT_STOP_REASON_CONTROL_PLANE_CLOSED => {
                        StopReason::ControlPlaneClosed
                    }
                    sys::kyclient_StopReason_KYCLIENT_STOP_REASON_CONTROL_PLANE_IO_ERROR => {
                        StopReason::ControlPlaneIoError
                    }
                    sys::kyclient_StopReason_KYCLIENT_STOP_REASON_CONTROL_PLANE_TIMEOUT => {
                        StopReason::ControlPlaneTimeout
                    }
                    sys::kyclient_StopReason_KYCLIENT_STOP_REASON_DECODE_ERROR => {
                        StopReason::DecodeError
                    }
                    _ => {
                        warn!("Fail to convert ConnectionLost {c_reason}");
                        return;
                    }
                };

                data.sink.on_event(Event::Stopped { reason });
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_DISPLAY_LIST_UPDATED => {
                let mut cdisplays: *mut Display = std::ptr::null_mut();
                let mut len: u32 = 0;

                let ret = unsafe {
                    sys::kyclient_event_unpack_display_list_updated(
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
                unsafe { sys::kyclient_event_free_displays(cdisplays) };
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_INPUT_SERVICE_CONNECTED => {
                data.sink.on_event(Event::InputServiceConnected);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_INPUT_SERVICE_DISCONNECTED => {
                data.sink.on_event(Event::InputServiceDisconnected);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_STREAMING_STARTED => {
                data.sink.on_event(Event::StreamingStarted);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_STREAMING_START_FAILED => {
                data.sink.on_event(Event::StreamingStartFailed);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_STREAMING_STOPPED => {
                data.sink.on_event(Event::StreamingStopped);
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_STREAMER_PIPELINE_EVENT => {
                let mut c_type = mem::MaybeUninit::uninit();
                let mut c_stage = mem::MaybeUninit::uninit();
                let mut c_event = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::kyclient_event_unpack_streamer_pipeline_event(
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
                    sys::kyclient_StreamerPipelineType_KYCLIENT_STREAMER_PIPELINE_TYPE_ENUMERATE_DISPLAYS => {
                        StreamerPipelineType::EnumerateDisplays
                    }
                    sys::kyclient_StreamerPipelineType_KYCLIENT_STREAMER_PIPELINE_TYPE_AUDIO => {
                        StreamerPipelineType::Audio
                    }
                    sys::kyclient_StreamerPipelineType_KYCLIENT_STREAMER_PIPELINE_TYPE_VIDEO => {
                        StreamerPipelineType::Video
                    }
                    _ => {
                        warn!("Fail to convert StreamerPipelineType {c_type}");
                        return;
                    }
                };

                let c_stage = c_stage.assume_init();
                let stage = match c_stage {
                    sys::kyclient_StreamerPipelineStage_KYCLIENT_STREAMER_PIPELINE_STAGE_PROCESS => {
                        StreamerPipelineStage::Process
                    }
                    sys::kyclient_StreamerPipelineStage_KYCLIENT_STREAMER_PIPELINE_STAGE_INITIALIZATION => {
                        StreamerPipelineStage::Initialization
                    }
                    sys::kyclient_StreamerPipelineStage_KYCLIENT_STREAMER_PIPELINE_STAGE_ACQUISITION => {
                        StreamerPipelineStage::Acquisition
                    }
                    sys::kyclient_StreamerPipelineStage_KYCLIENT_STREAMER_PIPELINE_STAGE_ENCODER => {
                        StreamerPipelineStage::Encoder
                    }
                    _ => {
                        warn!("Fail to convert StreamerPipelineStage {c_stage}");
                        return;
                    }
                };

                let c_event = c_event.assume_init();
                let event = match c_event {
                    sys::kyclient_StreamerPipelineEventType_KYCLIENT_STREAMER_PIPELINE_EVENT_TYPE_STOPPED => {
                        StreamerPipelineEventType::Stopped
                    }
                    sys::kyclient_StreamerPipelineEventType_KYCLIENT_STREAMER_PIPELINE_EVENT_TYPE_ERROR => {
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
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_AUDIO_VOLUME_CHANGED => {
                let mut volume = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::kyclient_event_unpack_audio_volume_changed(event, volume.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::AudioVolumeChanged");
                    return;
                }

                let volume = volume.assume_init();
                data.sink.on_event(Event::AudioVolumeChanged { volume });
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_AUDIO_MUTE_CHANGED => {
                let mut mute = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::kyclient_event_unpack_audio_mute_changed(event, mute.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::AudioMuteChanged");
                    return;
                }

                let mute = mute.assume_init();
                data.sink.on_event(Event::AudioMuteChanged { mute });
            }
            sys::kyclient_EventType_KYCLIENT_EVENT_TYPE_BITRATE_CHANGED => {
                let mut bitrate = mem::MaybeUninit::uninit();

                let ret = unsafe {
                    sys::kyclient_event_unpack_bitrate_changed(event, bitrate.as_mut_ptr())
                };
                if ret != 0 {
                    warn!("Fail to unpack EventType::BitrateChanged");
                    return;
                }

                let bitrate = bitrate.assume_init();
                data.sink.on_event(Event::BitrateChanged { bitrate });
            }
            _ => {
                error!("Invalid kyclient_EventType {}", event_type)
            }
        }
    }
}

fn to_cstr(s: &str) -> CString {
    CString::new(s.to_owned()).expect("Error: Unexpected null byte")
}

pub struct VideoPlayerConfig {
    ptr: *mut sys::kyclient_VideoPlayerConfig,
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
            let ret = sys::kyclient_video_player_config_default(window_handle, &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_software_decode(&self, software_decode: bool) {
        unsafe { sys::kyclient_video_player_config_set_software_decode(self.ptr, software_decode) }
    }

    pub fn set_video_buffer(&self, video_buffer: u32) {
        unsafe { sys::kyclient_video_player_config_set_video_buffer(self.ptr, video_buffer) }
    }
}

impl Drop for VideoPlayerConfig {
    fn drop(&mut self) {
        unsafe { sys::kyclient_video_player_config_unref(self.ptr) }
    }
}

pub struct VideoSessionConfig {
    ptr: *mut sys::kyclient_VideoSessionConfig,
}

impl VideoSessionConfig {
    pub fn default(display_id: u32, player: VideoPlayerConfig) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::kyclient_video_session_config_default(display_id, player.ptr, &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_codec(&self, codec: VideoCodec) {
        unsafe { sys::kyclient_video_session_config_set_codec(self.ptr, codec.to_u32()) }
    }

    pub fn set_yuv444(&self, yuv444: bool) {
        unsafe { sys::kyclient_video_session_config_set_yuv444(self.ptr, yuv444) }
    }

    pub fn set_intra_refresh(&self, intra_refresh: bool) {
        unsafe { sys::kyclient_video_session_config_set_intra_refresh(self.ptr, intra_refresh) }
    }
}

impl Drop for VideoSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::kyclient_video_session_config_unref(self.ptr) }
    }
}

pub struct AudioSessionConfig {
    ptr: *mut sys::kyclient_AudioSessionConfig,
}

impl AudioSessionConfig {
    pub fn new_default() -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::kyclient_audio_session_config_default(&mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_audio_buffer(&self, audio_buffer: u32) {
        unsafe { sys::kyclient_audio_session_config_set_audio_buffer(self.ptr, audio_buffer) }
    }
}

pub struct InputSessionConfig {
    ptr: *mut sys::kyclient_InputSessionConfig,
}

impl InputSessionConfig {
    pub fn default(pipeline: &Pipeline) -> Result<Self> {
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::kyclient_input_session_config_default(pipeline.to_ptr(), &mut ptr);
            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn gamepad_force_xbox360(&self, force: bool) {
        unsafe { sys::kyclient_input_session_config_gamepad_force_xbox360(self.ptr, force) };
    }

    pub fn clipboard(&self, enable: bool) {
        unsafe { sys::kyclient_input_session_config_clipboard(self.ptr, enable) };
    }
}

impl Drop for InputSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::kyclient_input_session_config_unref(self.ptr) }
    }
}

pub struct StreamingSessionConfig {
    ptr: *mut sys::kyclient_StreamingSessionConfig,
}

impl StreamingSessionConfig {
    pub fn default(protocol: StreamingProtocol) -> Result<Self> {
        let streaming_protocol = match protocol {
            #[cfg(feature = "rtp")]
            StreamingProtocol::Rtp => unsafe { sys::kyclient_streaming_protocol_rtp() },
            #[cfg(feature = "kymux")]
            StreamingProtocol::Kymux {
                video_protocol,
                audio_protocol,
            } => unsafe {
                sys::kyclient_streaming_protocol_kymux(
                    video_protocol.to_u32(),
                    audio_protocol.to_u32(),
                )
            },
        };

        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret = sys::kyclient_streaming_session_config_default(streaming_protocol, &mut ptr);
            sys::kyclient_streaming_protocol_unref(streaming_protocol);

            if ret == 0 {
                Ok(Self { ptr })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn get_bitrate(&self) -> u32 {
        unsafe { sys::kyclient_streaming_session_config_get_bitrate(self.ptr) }
    }

    pub fn set_bitrate(&self, bitrate: u32) {
        unsafe { sys::kyclient_streaming_session_config_set_bitrate(self.ptr, bitrate) }
    }

    pub fn add_video(&self, config: VideoSessionConfig) {
        unsafe { sys::kyclient_streaming_session_config_add_video(self.ptr, config.ptr) }

        mem::forget(config);
    }

    pub fn set_audio(&self, config: AudioSessionConfig) {
        unsafe { sys::kyclient_streaming_session_config_set_audio(self.ptr, config.ptr) }
    }

    pub fn set_input(&self, config: InputSessionConfig) {
        unsafe { sys::kyclient_streaming_session_config_set_input(self.ptr, config.ptr) }

        mem::forget(config);
    }
}

impl Drop for StreamingSessionConfig {
    fn drop(&mut self) {
        unsafe { sys::kyclient_streaming_session_config_unref(self.ptr) }
    }
}

fn raw_credentials(
    credentials: &AuthCredentials,
) -> Result<NonNull<sys::kyclient_AuthCredentials>> {
    let credentials = match credentials {
        AuthCredentials::Basic { username, password } => {
            let username = to_cstr(username);
            let password = to_cstr(password);
            unsafe {
                sys::kyclient_auth_credentials_new_basic(username.as_ptr(), password.as_ptr())
            }
        }
        AuthCredentials::Jwt { token } => {
            let token = to_cstr(token);
            unsafe { sys::kyclient_auth_credentials_new_jwt(token.as_ptr()) }
        }
        AuthCredentials::Oidc { token } => {
            let token_cstr = to_cstr(token);
            unsafe { sys::kyclient_auth_credentials_new_oidc(token_cstr.as_ptr()) }
        }
    };
    NonNull::new(credentials).ok_or(Error::Generic)
}

type TofuCallbackPtr = *mut Box<dyn Fn(TofuPrompt) -> bool + Send + Sync>;

pub struct ConnectConfig {
    ptr: *mut sys::kyclient_ConnectConfig,
    /// Stores the TOFU callback pointer for cleanup on drop.
    tofu_callback_ptr: Cell<Option<TofuCallbackPtr>>,
}

impl ConnectConfig {
    pub fn default(host: &str, credentials: &AuthCredentials) -> Result<Self> {
        let chost = to_cstr(host);

        let raw_creds = raw_credentials(credentials)?;
        unsafe {
            let mut ptr: *mut _ = std::mem::zeroed();
            let ret =
                sys::kyclient_connect_config_default(chost.as_ptr(), raw_creds.as_ptr(), &mut ptr);
            if ret == 0 {
                Ok(Self {
                    ptr,
                    tofu_callback_ptr: Cell::new(None),
                })
            } else {
                Err(Error::Generic)
            }
        }
    }

    pub fn set_tls_host(&self, tls_host: &str) -> Result<()> {
        let ctlshost = to_cstr(tls_host);

        let ret = unsafe { sys::kyclient_connect_config_set_tls_host(self.ptr, ctlshost.as_ptr()) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_tls_root_cert(&self, tls_root_cert: &str) -> Result<()> {
        let ctls_root_cert = to_cstr(tls_root_cert);

        let ret = unsafe {
            sys::kyclient_connect_config_set_tls_root_cert(self.ptr, ctls_root_cert.as_ptr())
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_streamer_port(&self, port: u16) {
        unsafe { sys::kyclient_connect_config_set_streamer_port(self.ptr, port) }
    }

    pub fn set_auto_reconnection(&self, enabled: bool) {
        unsafe { sys::kyclient_connect_config_set_auto_reconnection(self.ptr, enabled) }
    }

    /// Set TLS certificate fingerprint for pinning verification.
    /// The fingerprint should be in format "sha256:xx:xx:xx:..."
    pub fn set_tls_cert_fingerprint(&self, fingerprint: &str) -> Result<()> {
        let cfingerprint = to_cstr(fingerprint);

        let ret = unsafe {
            sys::kyclient_connect_config_set_tls_fingerprint(self.ptr, cfingerprint.as_ptr())
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    /// Enable TOFU (Trust-On-First-Use) verification with a callback.
    /// The callback receives a TofuPrompt and should return true to accept or false to reject.
    pub fn enable_tofu<F>(&self, callback: F) -> Result<()>
    where
        F: Fn(TofuPrompt) -> bool + Send + Sync + 'static,
    {
        // Free any previous callback
        if let Some(old_ptr) = self.tofu_callback_ptr.get() {
            unsafe {
                let _ = Box::from_raw(old_ptr);
            }
            self.tofu_callback_ptr.set(None);
        }

        // Store callback as a boxed trait object, then box again to get a thin pointer
        // This allows us to properly drop it later without knowing the concrete type
        let callback: Box<dyn Fn(TofuPrompt) -> bool + Send + Sync> = Box::new(callback);
        let callback_box = Box::new(callback);
        let callback_ptr = Box::into_raw(callback_box);

        let ret = unsafe {
            sys::kyclient_connect_config_enable_tofu(
                self.ptr,
                Some(tofu_callback_trampoline),
                callback_ptr as *mut c_void,
            )
        };

        if ret == 0 {
            // Store pointer for cleanup on drop
            self.tofu_callback_ptr.set(Some(callback_ptr));
            Ok(())
        } else {
            // Clean up callback on error
            unsafe {
                let _ = Box::from_raw(callback_ptr);
            }
            Err(Error::Generic)
        }
    }

    /// Skip TLS verification (DANGER - for testing only).
    pub fn tls_skip_verification(&self) {
        unsafe { sys::kyclient_connect_config_skip_tls_verification(self.ptr) }
    }
}

/// Trampoline function for TOFU callbacks
unsafe extern "C" fn tofu_callback_trampoline(
    prompt: *const sys::kyclient_TofuPrompt,
    userdata: *mut c_void,
) -> bool {
    // userdata points to Box<Box<dyn Fn(TofuPrompt) -> bool>>
    let callback = &*(userdata as *const Box<dyn Fn(TofuPrompt) -> bool>);
    let prompt = &*prompt;

    // Convert C prompt to Rust TofuPrompt
    let rust_prompt = match prompt.prompt_type {
        sys::kyclient_TofuPromptType_KYCLIENT_TOFU_PROMPT_TYPE_UNKNOWN_HOST => {
            let hostname = CStr::from_ptr(prompt.hostname)
                .to_str()
                .unwrap_or("")
                .to_string();
            let fingerprint = CStr::from_ptr(prompt.fingerprint)
                .to_str()
                .unwrap_or("")
                .to_string();
            TofuPrompt::UnknownHost {
                hostname,
                fingerprint,
            }
        }
        sys::kyclient_TofuPromptType_KYCLIENT_TOFU_PROMPT_TYPE_CERTIFICATE_CHANGED => {
            let hostname = CStr::from_ptr(prompt.hostname)
                .to_str()
                .unwrap_or("")
                .to_string();
            let expected_fingerprint = CStr::from_ptr(prompt.expected_fingerprint)
                .to_str()
                .unwrap_or("")
                .to_string();
            let actual_fingerprint = CStr::from_ptr(prompt.actual_fingerprint)
                .to_str()
                .unwrap_or("")
                .to_string();
            TofuPrompt::CertificateChanged {
                hostname,
                expected_fingerprint,
                actual_fingerprint,
            }
        }
        _ => return false, // Unknown prompt type
    };

    callback(rust_prompt)
}

impl Drop for ConnectConfig {
    fn drop(&mut self) {
        // Free the TOFU callback if one was registered and not transferred to Client
        if let Some(callback_ptr) = self.tofu_callback_ptr.get() {
            unsafe {
                let _ = Box::from_raw(callback_ptr);
            }
        }
        unsafe { sys::kyclient_connect_config_unref(self.ptr) }
    }
}

pub struct Client {
    ptr: *mut sys::kyclient_Client,
    metrics_cb: Cell<Option<Box<Box<MetricsCallback>>>>,
    /// TOFU callback pointer transferred from ConnectConfig.
    /// Must stay alive for the lifetime of the client.
    tofu_callback_ptr: Cell<Option<TofuCallbackPtr>>,
}

impl Client {
    pub fn new(connect_config: &ConnectConfig, metrics: bool) -> Result<Self> {
        let ptr = unsafe { sys::kyclient_client_new(connect_config.ptr, metrics) };
        if !ptr.is_null() {
            // Transfer TOFU callback ownership from ConnectConfig to Client.
            // The callback must stay alive for the lifetime of the Client since
            // the TofuVerifier holds a reference to it through the C API.
            let tofu_callback_ptr = connect_config.tofu_callback_ptr.get();
            connect_config.tofu_callback_ptr.set(None); // Prevent double-free in ConnectConfig::drop

            Ok(Self {
                ptr,
                metrics_cb: Cell::new(None),
                tofu_callback_ptr: Cell::new(tofu_callback_ptr),
            })
        } else {
            Err(Error::Generic)
        }
    }

    pub fn stop(&self) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_stop(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn disconnect_and_stop(&self, stop_session: bool) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_disconnect_and_stop(self.ptr, stop_session) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn connect(&self) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_connect(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn update_credentials(&self, credentials: &AuthCredentials) -> Result<()> {
        let raw_creds = raw_credentials(credentials)?;
        let ret = unsafe { sys::kyclient_client_update_credentials(self.ptr, raw_creds.as_ptr()) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn disconnect(&self) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_disconnect(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn start_streaming(&self, session_config: &StreamingSessionConfig) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_start_streaming(self.ptr, session_config.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn stop_streaming(&self) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_stop_streaming(self.ptr) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_audio_set_volume(self.ptr, volume) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_audio_set_mute(self.ptr, mute) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_bitrate(&self, bitrate: u32) -> Result<()> {
        let ret = unsafe { sys::kyclient_client_set_bitrate(self.ptr, bitrate) };
        if ret == 0 {
            Ok(())
        } else {
            Err(Error::Generic)
        }
    }

    pub fn set_event_handler(&self, sink: Box<dyn EventSink>) -> Result<()> {
        let handler = sys::kyclient_EventHandler {
            on_event: Some(EventSinkData::on_event_cb),
            userdata: Box::into_raw(Box::new(EventSinkData { sink })) as *mut _,
        };

        let ret = unsafe { sys::kyclient_client_set_event_handler(self.ptr, handler) };
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
                sys::kyclient_client_set_metrics_callback(self.ptr, Some(metrics_cb), userdata);
            }
        } else {
            unsafe {
                sys::kyclient_client_set_metrics_callback(self.ptr, None, std::ptr::null_mut());
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if self.metrics_cb.take().is_some() {
            self.set_metrics_callback(None);
        }

        // Free the TOFU callback if one was transferred from ConnectConfig
        if let Some(callback_ptr) = self.tofu_callback_ptr.get() {
            unsafe {
                let _ = Box::from_raw(callback_ptr);
            }
        }

        // Terminate the background thread. Safe to call even if already stopped.
        let _ = self.stop();
    }
}

extern "C" fn log_cb(
    level: sys::kyclient_LogLevel,
    target: *const libc::c_char,
    line: *const libc::c_char,
    _userdata: *mut libc::c_void,
) {
    let target = unsafe { CStr::from_ptr(target).to_str().unwrap() };
    let line = unsafe { CStr::from_ptr(line).to_str().unwrap() };

    let level = match level {
        sys::kyclient_LogLevel_KYCLIENT_LOG_LEVEL_ERROR => Some(log::Level::Error),
        sys::kyclient_LogLevel_KYCLIENT_LOG_LEVEL_WARN => Some(log::Level::Warn),
        sys::kyclient_LogLevel_KYCLIENT_LOG_LEVEL_INFO => Some(log::Level::Info),
        sys::kyclient_LogLevel_KYCLIENT_LOG_LEVEL_DEBUG => Some(log::Level::Debug),
        sys::kyclient_LogLevel_KYCLIENT_LOG_LEVEL_TRACE => Some(log::Level::Trace),
        _ => None,
    };

    log::log!(target: target, level.unwrap(), "{}", line);
}

pub fn init() {
    unsafe {
        sys::kyclient_set_log_cb(Some(log_cb), std::ptr::null_mut());
        sys::kyclient_init_crypto();
    }
}

fn to_c_metric(metric: &Metric) -> sys::kyclient_Metric {
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
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_ACQUIRED
                }
                VideoMetricData::FrameEncoding => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODING
                }
                VideoMetricData::FrameEncoded => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODED
                }
                VideoMetricData::FrameSent => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_SENT
                }
                VideoMetricData::FrameReceived => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_RECEIVED
                }
                VideoMetricData::FrameDecoding => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_DECODING
                }
                VideoMetricData::FrameDecoded => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_DECODED
                }
                VideoMetricData::FramePrepared => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_PREPARED
                }
                VideoMetricData::FrameDisplayed => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_DISPLAYED
                }
                VideoMetricData::FrameSkipped => {
                    sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_SKIPPED
                }
            };

            sys::kyclient_Metric {
                type_: sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_VIDEO,
                data: sys::kyclient_MetricData {
                    video: sys::kyclient_VideoMetric {
                        source_id: metric.source_id,
                        pts: metric.pts,
                        ts: metric.ts,
                        data,
                    },
                },
            }
        }
        Metric::NetworkLocal(metric) => sys::kyclient_Metric {
            type_: sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_NETWORK_LOCAL,
            data: sys::kyclient_MetricData {
                network_local: sys::kyclient_NetworkStats {
                    rtt_micros: map_value(metric.rtt_micros),
                    packets_lost: map_value(metric.packets_lost),
                    dropped_packets: map_value(metric.dropped_packets),
                },
            },
        },
        Metric::NetworkPing(metric) => sys::kyclient_Metric {
            type_: sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_NETWORK_PING,
            data: sys::kyclient_MetricData {
                network_ping: sys::kyclient_NetworkPing {
                    offset_micros: metric.offset_micros,
                    delay_micros: metric.delay_micros,
                },
            },
        },
        Metric::NetworkRemote(metric) => sys::kyclient_Metric {
            type_: sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_NETWORK_REMOTE,
            data: sys::kyclient_MetricData {
                network_local: sys::kyclient_NetworkStats {
                    rtt_micros: map_value(metric.rtt_micros),
                    packets_lost: map_value(metric.packets_lost),
                    dropped_packets: map_value(metric.dropped_packets),
                },
            },
        },
    }
}

// Cannot implement From due to the orphan rule:
//  - native Rust structures are in kyclient-common
//  - Rust wrappers generated from C code are un kyclient-sys

fn from_metric(metric: &sys::kyclient_Metric) -> Metric {
    match metric.type_ {
        sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_VIDEO => {
            let video_metric = from_video_metric(unsafe { &metric.data.video });
            Metric::Video(video_metric)
        }
        sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_NETWORK_LOCAL => {
            let network_stats = from_network_stats(unsafe { &metric.data.network_local });
            Metric::NetworkLocal(network_stats)
        }
        sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_NETWORK_PING => {
            let network_ping = from_network_ping(unsafe { &metric.data.network_ping });
            Metric::NetworkPing(network_ping)
        }
        sys::kyclient_MetricType_KYCLIENT_METRIC_TYPE_NETWORK_REMOTE => {
            let network_stats = from_network_stats(unsafe { &metric.data.network_remote });
            Metric::NetworkRemote(network_stats)
        }
        v => panic!("Unexpected metric type: {v}"),
    }
}

fn from_video_metric(video_metric: &sys::kyclient_VideoMetric) -> VideoMetric {
    VideoMetric {
        source_id: video_metric.source_id,
        pts: video_metric.pts,
        ts: video_metric.ts,
        data: from_video_metric_data(&video_metric.data),
    }
}

fn from_video_metric_data(video_metric_data: &sys::kyclient_VideoMetricData) -> VideoMetricData {
    match *video_metric_data {
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_ACQUIRED => {
            VideoMetricData::FrameAcquired
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODING => {
            VideoMetricData::FrameEncoding
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_ENCODED => {
            VideoMetricData::FrameEncoded
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_SENT => {
            VideoMetricData::FrameSent
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_RECEIVED => {
            VideoMetricData::FrameReceived
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_DECODING => {
            VideoMetricData::FrameDecoding
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_DECODED => {
            VideoMetricData::FrameDecoded
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_PREPARED => {
            VideoMetricData::FramePrepared
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_DISPLAYED => {
            VideoMetricData::FrameDisplayed
        }
        sys::kyclient_VideoMetricData_KYCLIENT_VIDEO_METRIC_DATA_FRAME_SKIPPED => {
            VideoMetricData::FrameSkipped
        }
        v => panic!("Unexpected video metric data type: {v}"),
    }
}

fn from_network_stats(network_stats: &sys::kyclient_NetworkStats) -> NetworkStats {
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

fn from_network_ping(network_ping: &sys::kyclient_NetworkPing) -> NetworkPing {
    NetworkPing {
        offset_micros: network_ping.offset_micros,
        delay_micros: network_ping.delay_micros,
    }
}

extern "C" fn metrics_cb(metric: *const sys::kyclient_Metric, userdata: *mut c_void) {
    let cb: &mut MetricsCallback = unsafe { &mut **(userdata as *mut Box<MetricsCallback>) };
    let metric = from_metric(unsafe { &*metric });
    cb(metric);
}

pub struct MetricsJSONProducer {
    ptr: AtomicPtr<sys::kyclient_metrics_json_producer>,
}

impl Default for MetricsJSONProducer {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsJSONProducer {
    pub fn new() -> Self {
        let ptr = unsafe { sys::kyclient_metrics_json_producer_new() };
        Self {
            ptr: AtomicPtr::new(ptr),
        }
    }

    pub fn push(&self, metric: &Metric) -> Vec<Vec<u8>> {
        let ptr = self.ptr.load(atomic::Ordering::Relaxed);
        let metric = to_c_metric(metric);

        let c_json_list =
            unsafe { sys::kyclient_metrics_json_producer_push(ptr, &metric as *const _) };

        let c_values = unsafe {
            let mut c_values = MaybeUninit::uninit();
            let mut c_values_count = MaybeUninit::uninit();

            sys::kyclient_metrics_json_values(
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
            sys::kyclient_metrics_json_unref(c_json_list);
        }

        values
    }
}

impl Drop for MetricsJSONProducer {
    fn drop(&mut self) {
        let ptr = self.ptr.load(atomic::Ordering::Relaxed);
        unsafe { sys::kyclient_metrics_json_producer_unref(ptr) };
    }
}

/// Add a host to the known_hosts file.
pub fn known_hosts_add(hostname: &str, fingerprint: &str) -> Result<()> {
    let chostname = to_cstr(hostname);
    let cfingerprint = to_cstr(fingerprint);

    let ret = unsafe { sys::kyclient_known_hosts_add(chostname.as_ptr(), cfingerprint.as_ptr()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(Error::Generic)
    }
}

/// Remove a host from the known_hosts file.
/// Returns Ok(true) if removed, Ok(false) if not found.
pub fn known_hosts_remove(hostname: &str) -> Result<bool> {
    let chostname = to_cstr(hostname);

    let ret = unsafe { sys::kyclient_known_hosts_remove(chostname.as_ptr()) };
    match ret {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error::Generic),
    }
}

/// List all hosts in the known_hosts file.
/// Returns a vector of (hostname, fingerprint) pairs.
pub fn known_hosts_list() -> Result<Vec<(String, String)>> {
    let mut entries: *mut sys::kyclient_KnownHostEntry = std::ptr::null_mut();
    let mut count: usize = 0;

    let ret = unsafe { sys::kyclient_known_hosts_list(&mut entries, &mut count) };
    if ret != 0 {
        return Err(Error::Generic);
    }

    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        unsafe {
            let entry = &*entries.add(i);
            let hostname = CStr::from_ptr(entry.hostname)
                .to_str()
                .unwrap_or("")
                .to_string();
            let fingerprint = CStr::from_ptr(entry.fingerprint)
                .to_str()
                .unwrap_or("")
                .to_string();
            result.push((hostname, fingerprint));
        }
    }

    unsafe {
        sys::kyclient_known_hosts_list_free(entries, count);
    }

    Ok(result)
}
