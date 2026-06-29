// Project Kyber: error.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::mem::MaybeUninit;
use std::ptr;

use log::warn;
use windows_sys::{
    Win32::{
        Foundation::*,
        System::{Diagnostics::Debug::*, SystemServices::*},
    },
    core::*,
};

use crate::{Error, Result};

#[inline]
pub(crate) fn makelangid(p: u32, s: u32) -> u32 {
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

    let mut buffer: MaybeUninit<PWSTR> = MaybeUninit::uninit();

    let s = unsafe {
        let len = FormatMessageW(
            FORMAT_MESSAGE_ALLOCATE_BUFFER
                | FORMAT_MESSAGE_FROM_SYSTEM
                | FORMAT_MESSAGE_IGNORE_INSERTS,
            ptr::null_mut(),
            error_code,
            makelangid(LANG_NEUTRAL, SUBLANG_DEFAULT),
            #[allow(clippy::crosspointer_transmute)]
            std::mem::transmute::<*mut *mut u16, *mut u16>(buffer.as_mut_ptr()),
            0,
            ptr::null_mut(),
        );

        if len > 0 {
            let buffer = buffer.assume_init();
            let s = String::from_utf16(std::slice::from_raw_parts(buffer, len as usize));
            LocalFree(buffer as _);

            if let Err(err) = &s {
                warn!("Failed to convert Win32 error 0x{error_code:X}: {err:?}");
            }

            s.ok()
        } else {
            warn!("FormatMessageW() failed: {:X}", GetLastError());

            None
        }
    };

    if let Some(s) = s {
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
