use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO Error: {source:?}")]
    IO {
        #[from]
        source: std::io::Error,
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
    #[error("Got an unexpected response from AvServer")]
    AvServerUnexpectedResponse,
}

pub type Result<T> = std::result::Result<T, Error>;
