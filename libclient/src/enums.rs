use serde::{Deserialize, Serialize};

use crate::host_events;
use crate::{PlayerConfig, StreamingSessionConfig};

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
#[allow(dead_code)]
pub(crate) enum Message {
    Connect,
    Disconnect,

    ControlPlaneClosed {
        reason: ControlPlaneCloseReason,
    },

    StartStreamingSession {
        streaming_config: StreamingSessionConfig,
        player_config: PlayerConfig,
    },
    StopStreamingSession,

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    ControlPlaneConnected {
        displays: Vec<Display>,
    },
    ControlPlaneConnectFailed,
    Stopped {
        reason: StopReason,
    },
    DisplayListUpdated {
        displays: Vec<Display>,
    },
    StreamingStarted,
    StreamingStopped,
    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
    },
}
