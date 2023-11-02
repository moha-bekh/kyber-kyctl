use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO Error: {source:?}")]
    IO {
        #[from]
        source: std::io::Error,
    },
    #[error("kymux error: {source:?}")]
    KymuxError {
        #[from]
        source: kymux::Error,
    },
    #[error("libkypc error: {source:?}")]
    LibypcError {
        #[from]
        source: libkypc::Error,
    },
    #[error("Streamer already started")]
    StreamerStarted,
    #[error("Streamer already stopped")]
    StreamerStopped,
    #[error("Kymux has no connection")]
    KymuxNotConnected,
    #[error("Got an unexpected response from AvServer")]
    AvServerUnexpectedResponse,
    #[error("Couldn't get a receiver from AvServer")]
    AvServerNoReceiver,
    #[error("Couldn't open file {path}")]
    FileNotFound { path: PathBuf },
    #[error("Invalid TLS file")]
    TlsInvalidFile,
    #[error("Invalid TLS certificate: {source:?}")]
    TlsInvalidCertificate { source: rustls::Error },
    #[error("rustls error: {source:?}")]
    RustlsError {
        #[from]
        source: rustls::Error,
    },
    #[error("Actix Web error: {source:?}")]
    ActixError {
        #[from]
        source: actix_web::Error,
    },
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
}

pub type Result<T> = std::result::Result<T, Error>;
