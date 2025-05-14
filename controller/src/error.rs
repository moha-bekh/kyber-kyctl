use std::path::PathBuf;

use kymux::kyproto;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO Error: {source:?}")]
    IO {
        #[from]
        source: std::io::Error,
    },
    #[error("kymux error: {source:?}")]
    Kymux {
        #[from]
        source: kymux::Error,
    },
    #[error("kyproto protocol error: {source:?}")]
    KyProtoProtocol {
        #[from]
        source: kyproto::ProtocolError,
    },
    #[error("libkypc error: {source:?}")]
    Libypc {
        #[from]
        source: libkypc::Error,
    },
    #[error("Streamer already started")]
    StreamerStarted,
    #[error("Streamer already stopped")]
    StreamerStopped,
    #[error("Control plane not opened")]
    ControlPlaneNotOpened,
    #[error("Data plane already opened")]
    DataPlaneAlreadyOpened,
    #[error("Kymux accept() timeout")]
    KymuxAcceptTimeout,
    #[error("Kymux isn't listening")]
    KymuxNotListening,
    #[error("Got an unexpected response from AvServer")]
    AvServerUnexpectedResponse,
    #[error("Couldn't get a receiver from AvServer")]
    AvServerNoReceiver,
    #[error("Couldn't open file {path}")]
    FileNotFound { path: PathBuf },
    #[error("Invalid TLS file")]
    TlsInvalidFile,
    #[error("rustls error: {source:?}")]
    Rustls {
        #[from]
        source: rustls::Error,
    },
    #[error("rustls default provider failed to initialize")]
    RustlsDefaultProviderInit,
    #[error("rcgen error: {source:?}")]
    RcGen {
        #[from]
        source: rcgen::Error,
    },
    #[error("Operation on date overflow/underflow")]
    DateOperationOverflow,
    #[error("Actix Web error: {0}")]
    Actix(String),
    #[error("Thread panicked")]
    ThreadPanicked,
    #[error("Channel receive failed (channel closed)")]
    ChannelClosedRx,
    #[error("Channel send failed (channel closed)")]
    ChannelClosedTx,
    #[error("DisplayList empty")]
    DisplayListEmpty,
    #[error("No Websocket router")]
    WsNoRouter,
    #[error("Websocket component {component} not ready")]
    WsComponentNotReady { component: String },
    #[error("Websocket got invalid endpoint {endpoint_id:X} from process")]
    WsInvalidEndpoint { endpoint_id: u16 },
    #[error("Config parse failed")]
    ConfigParseFail,
    #[error("Config load failed: {reason:?}")]
    ConfigLoadFail { reason: toml::de::Error },
    #[error("No statemachine proxy")]
    NotStateMachineProxy,
}

pub type Result<T> = std::result::Result<T, Error>;

// actix_web::Error is not Send, we want our error::Error to be Send
impl From<actix_web::Error> for Error {
    fn from(err: actix_web::Error) -> Self {
        Error::Actix(err.to_string())
    }
}
