use std::path::PathBuf;

use crate::platform;

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
    NoPlayer,
    NoAudioPlayer,
    Input(platform::InputError),
    #[cfg(feature = "kymux")]
    Kymux(kymux::Error),
    KymuxNotStarted,
    #[cfg(feature = "kymux")]
    KymuxReadyFailed {
        reason: String,
    },
    #[cfg(feature = "kymux")]
    KymuxSendFailed {
        reason: String,
    },
    #[cfg(feature = "desktop")]
    TaskJoin(tokio::task::JoinError),
    FileNotFound(PathBuf),
    TlsInvalidFile,
    InvalidState,

    #[cfg(feature = "web")]
    JsError(wasm_bindgen::JsValue),
    #[cfg(feature = "web")]
    JsElementNotFound {
        name: String,
    },
    #[cfg(feature = "web")]
    JsCastFailed {
        to: String,
    },
    #[cfg(feature = "web")]
    JsDomInvalid,
    #[cfg(feature = "web")]
    NoValidVideoDecoderConfig,
    #[cfg(feature = "web")]
    SharedArrayBufferUndefined,

    // Player errors
    Player(PlayerError),
    // Host service errors
    HttpClient(reqwest::Error),
    HttpQueryTimeout,
    HttpQueryFailed {
        status: u16,
    },
    WsConnectFailed,
    DataPlaneConnectFailed,
    DeserializationFailed,
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

impl From<platform::InputError> for Error {
    fn from(err: platform::InputError) -> Error {
        Error::Input(err)
    }
}

#[cfg(feature = "kymux")]
impl From<kymux::Error> for Error {
    fn from(err: kymux::Error) -> Error {
        Error::Kymux(err)
    }
}

#[cfg(feature = "desktop")]
impl From<tokio::task::JoinError> for Error {
    fn from(err: tokio::task::JoinError) -> Error {
        Error::TaskJoin(err)
    }
}

#[cfg(feature = "web")]
impl From<wasm_bindgen::JsValue> for Error {
    fn from(err: wasm_bindgen::JsValue) -> Error {
        Error::JsError(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
