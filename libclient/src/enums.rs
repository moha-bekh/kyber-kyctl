#[derive(Debug)]
pub enum Message {
    Start,
    Stop,

    PlayerStarted,
    PlayerStopped,

    InputServiceConnected,
    InputServiceDisconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Event {
    Stopped,
}
