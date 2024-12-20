use serde::{Deserialize, Serialize};

use crate::host_events;
use crate::{
    StreamerPipelineEventType, StreamerPipelineStage, StreamerPipelineType, StreamingSessionConfig,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[repr(C)]
pub struct Display {
    pub id: u32,
    pub height: i32,
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug)]
pub enum ControlPlaneCloseReason {
    Eof,
    Closed {
        was_clean: bool,
        code: Option<u16>,
        reason: Option<String>,
    },
    Timeout,
}

#[derive(Clone, Debug)]
pub enum DataPlaneCloseReason {
    Closed,
    Error,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) enum Message {
    Connect,
    Disconnect,

    ControlPlaneClosed {
        reason: ControlPlaneCloseReason,
    },

    DataPlaneClosed {
        reason: DataPlaneCloseReason,
    },

    StartStreamingSession {
        config: StreamingSessionConfig,
    },
    StopStreamingSession,

    AudioSetVolume {
        volume: f32,
    },
    AudioSetMute {
        mute: bool,
    },
    AudioVolumeChanged {
        volume: f32,
    },
    AudioMuteChanged {
        mute: bool,
    },

    SetBitrate {
        bitrate: u32,
    },
    BitrateChanged {
        bitrate: u32,
    },

    PlayerStarted,
    PlayerStopped,

    InputServiceConnected,
    InputServiceDisconnected,

    DisplayListUpdated {
        displays: Vec<Display>,
    },

    StreamerPipelineEvent(host_events::StreamerPipelineEvent),

    VideoDecodeError,
    AudioDecodeError,

    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
        frame_coded_width: i32,
        frame_coded_height: i32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Requested,
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[derive(Clone, Debug, PartialEq)]
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
    AudioVolumeChanged {
        volume: f32,
    },
    AudioMuteChanged {
        mute: bool,
    },
    BitrateChanged {
        bitrate: u32,
    },
    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
        frame_coded_width: i32,
        frame_coded_height: i32,
    },
    StreamerPipelineEvent {
        type_: StreamerPipelineType,
        stage: StreamerPipelineStage,
        event: StreamerPipelineEventType,
    },
}
