use std::path::PathBuf;

#[derive(Debug)]
pub enum PlayerError {
    InstanceCreateFailed,
    MediaCreateFailed,
    MediaPlayerCreateFailed,
    PlayFailed,
    AlreadyStopped,
    EvtRegisterFailed,
}

#[derive(Debug)]
pub enum Error {
    IO(std::io::Error),
    InvalidAddress {
        addr: String,
    },
    FailToResolveAddress {
        addr: String,
    },
    InvalidUnicodeStr,
    ChannelSend,
    Rejected,
    ThreadPanicked,
    NoPipeline,
    NoClient,
    NoInputService,
    Input(libkynput_rs::Error),
    #[cfg(feature = "kymux")]
    Kymux(kymux::Error),
    KymuxNotStarted,
    TaskJoin(tokio::task::JoinError),
    FileNotFound(PathBuf),
    TlsInvalidFile,
    InvalidState,

    // Player errors
    Player(PlayerError),
    // Host service errors
    HttpClient(reqwest::Error),
    StreamStartRejected,
    StreamStopRejected,
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::IO(err)
    }
}

impl From<PlayerError> for Error {
    fn from(err: PlayerError) -> Error {
        Error::Player(err)
    }
}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Error {
        Error::HttpClient(err)
    }
}

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error {
    fn from(_err: tokio::sync::mpsc::error::SendError<T>) -> Error {
        Error::ChannelSend
    }
}

impl<T> From<std::sync::PoisonError<T>> for Error {
    fn from(_: std::sync::PoisonError<T>) -> Error {
        Error::ThreadPanicked
    }
}

impl From<libkynput_rs::Error> for Error {
    fn from(err: libkynput_rs::Error) -> Error {
        Error::Input(err)
    }
}

impl From<tokio::task::JoinError> for Error {
    fn from(err: tokio::task::JoinError) -> Error {
        Error::TaskJoin(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
