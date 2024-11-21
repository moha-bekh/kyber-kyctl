use std::ptr;

use log::warn;
use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};

use super::error::{check_win32_error, last_win32_error, Result};
use super::Handle;

pub struct Event {
    name: String,
    handle: Handle,
}

impl Event {
    pub fn new(name: &str, inheritable: bool) -> Result<Self> {
        let attrs = SECURITY_ATTRIBUTES {
            nLength: 0,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: if inheritable { TRUE } else { FALSE },
        };

        let handle = unsafe { CreateEventA(&attrs as *const _, FALSE, FALSE, ptr::null()) };
        check_win32_error!(CreateEventA, handle != 0)?;

        Ok(Self {
            name: name.into(),
            handle: Handle::new(handle),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn handle(&self) -> HANDLE {
        self.handle.handle()
    }
}
