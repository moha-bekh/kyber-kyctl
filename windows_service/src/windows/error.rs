use std::ptr;

use windows_sys::{
    core::*,
    Win32::{
        Foundation::*,
        System::{Diagnostics::Debug::*, SystemServices::*},
    },
};

#[inline]
pub fn makelangid(p: u32, s: u32) -> u32 {
    (s << 10) | p
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct Win32Error {
    func: &'static str,
    pub os_error: u32,
    os_str_error: Option<String>,

    file: &'static str,
    line: u32,
}

pub(crate) fn last_win32_error(func: &'static str, file: &'static str, line: u32) -> Result<()> {
    let error_code = unsafe { GetLastError() };
    if error_code == 0 {
        return Ok(());
    }

    let buffer: PWSTR = ptr::null_mut();

    let s = unsafe {
        let len = FormatMessageW(
            FORMAT_MESSAGE_ALLOCATE_BUFFER
                | FORMAT_MESSAGE_FROM_SYSTEM
                | FORMAT_MESSAGE_IGNORE_INSERTS,
            ptr::null_mut(),
            error_code,
            makelangid(LANG_NEUTRAL, SUBLANG_DEFAULT),
            buffer,
            0,
            ptr::null_mut(),
        );

        let s = String::from_utf16(std::slice::from_raw_parts(buffer, len as usize));
        LocalFree(buffer as _);

        s
    };

    if let Ok(s) = s {
        Err(Error::Win32(Win32Error {
            func,
            os_error: error_code,
            os_str_error: Some(s.trim().to_string()),
            file,
            line,
        }))
    } else {
        Err(Error::Win32(Win32Error {
            func,
            os_error: error_code,
            os_str_error: None,
            file,
            line,
        }))
    }
}

macro_rules! check_win32_error {
    ($func_name:ident, $expression:expr) => {
        if !$expression {
            let res = last_win32_error(stringify!($func_name), std::file!(), std::line!());
            if res.is_ok() {
                warn!(
                    "{}:{}: {} didn't return an error",
                    std::file!(),
                    std::line!(),
                    stringify!($func_name)
                );
            }

            res
        } else {
            Ok(())
        }
    };
}

pub(crate) use check_win32_error;

#[derive(Debug)]
pub enum Error {
    Rejected,

    ConfigParseFail,
    ConfigLoadFail { reason: toml::de::Error },

    Win32(Win32Error),
    Idd(kidd_client::Error),
}

impl From<kidd_client::Error> for Error {
    fn from(err: kidd_client::Error) -> Self {
        Self::Idd(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
