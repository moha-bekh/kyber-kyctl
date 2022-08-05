#[derive(Debug)]
pub enum Error {
    Rejected,
    IO(std::io::Error),
    StreamerStarted,
    StreamerStopped,
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::IO(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
