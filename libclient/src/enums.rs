use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
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

    PlayerStarted,
    PlayerStopped,

    InputServiceConnected,
    InputServiceDisconnected,

    DisplayListUpdated { displays: Vec<Display> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Started { displays: Vec<Display> },
    Stopped,
}
