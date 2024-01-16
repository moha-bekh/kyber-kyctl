use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[repr(C)]
pub struct Display {
    pub id: u32,
    pub height: i32,
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug)]
pub enum Message {
    Start,
    Stop,

    ConnectionLost,

    PlayerStarted,
    PlayerStopped,

    InputServiceConnected,
    InputServiceDisconnected,

    DisplayListUpdated {
        displays: Vec<Display>,
    },

    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Requested,
    ConnectionLost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Started {
        displays: Vec<Display>,
    },
    Stopped {
        reason: StopReason,
    },

    DisplayListUpdated {
        displays: Vec<Display>,
    },

    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
    },
}
