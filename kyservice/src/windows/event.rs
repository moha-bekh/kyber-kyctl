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
use std::sync::Arc;

use log::warn;
use windows_sys::Win32::{Foundation::*, Security::*, System::Threading::*};

use super::error::{check_win32_error, last_win32_error, Result};
use super::Handle;

#[derive(Clone, Debug)]
pub struct Event {
    name: String,
    handle: Arc<Handle>,
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
            handle: Arc::new(Handle::new(handle)),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn handle(&self) -> HANDLE {
        self.handle.handle()
    }

    pub fn set(&self) -> Result<()> {
        let ret = unsafe { SetEvent(self.handle.handle()) };
        check_win32_error!(SetEvent, ret != 0)?;

        Ok(())
    }

    pub fn reset(&self) -> Result<()> {
        let ret = unsafe { ResetEvent(self.handle.handle()) };
        check_win32_error!(ResetEvent, ret != 0)?;

        Ok(())
    }
}
