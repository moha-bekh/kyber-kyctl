// Project Kyber: event.rs
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

use std::ptr;
use std::time::Duration;

use log::warn;
use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};

use crate::platform::error::{check_win32_error, last_win32_error};
use crate::{Error, Result};

pub(crate) struct Event {
    handle: HANDLE,
}

impl Event {
    pub(crate) fn new() -> Result<Self> {
        let attrs = SECURITY_ATTRIBUTES {
            nLength: 0,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: FALSE,
        };

        let handle = unsafe { CreateEventA(&attrs as *const _, FALSE, FALSE, ptr::null()) };
        check_win32_error!(CreateEventA, handle != 0)?;

        Ok(Self { handle })
    }

    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }

    pub(crate) fn set(&self) -> Result<()> {
        let ret = unsafe { SetEvent(self.handle) };
        if ret == 0 {
            last_win32_error("SetEvent", std::file!(), std::line!())?;
        }

        Ok(())
    }

    pub(crate) fn wait(&self, timeout: Duration) -> Result<()> {
        let ret = unsafe { WaitForSingleObject(self.handle, timeout.as_millis() as _) };
        match ret {
            WAIT_OBJECT_0 => Ok(()),
            WAIT_TIMEOUT => Err(Error::EventTimedout),
            WAIT_FAILED => {
                let win32_err = last_win32_error("WaitForSingleObject()", file!(), line!());
                if let Err(Error::Win32(err)) = win32_err {
                    Err(Error::EventWaitError(err))
                } else {
                    warn!("WaitForSingleObject() invalid returned WAIT_FAILED");
                    Ok(())
                }
            }
            _ => Err(Error::EventUnexpectedResult(ret)),
        }
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}
