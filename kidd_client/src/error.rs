#[derive(Debug)]
pub enum Error {
    NotImplemented,
    Rejected,
    InvalidReadSize,
    #[cfg(target_os = "windows")]
    EventTimedout,
    #[cfg(target_os = "windows")]
    EventUnexpectedResult(u32),
    #[cfg(target_os = "windows")]
    DriverNotReady,
    #[cfg(target_os = "windows")]
    EventWaitError(crate::platform::Win32Error),
    #[cfg(target_os = "windows")]
    Win32(crate::platform::Win32Error),
    #[cfg(target_os = "windows")]
    Win32CallRejected,
}

pub type Result<T> = std::result::Result<T, Error>;
