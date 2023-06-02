mod error;
mod modes;

pub use error::{Error, Result};
pub use modes::MonitorMode;

#[cfg(target_os = "windows")]
#[path = "windows/mod.rs"]
mod platform;

#[cfg(not(target_os = "windows"))]
mod platform {
    use crate::error::{Error, Result};

    pub struct DeviceInterface;

    impl DeviceInterface {
        pub fn plug_monitor(&self) -> Result<()> {
            Err(Error::NotImplemented)
        }
    }

    pub struct Driver;

    impl Driver {
        pub fn open() -> Result<DeviceInterface> {
            Err(Error::NotImplemented)
        }
    }
}

pub type DeviceInterface = platform::DeviceInterface;
pub type Driver = platform::Driver;
