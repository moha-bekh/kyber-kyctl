// Project Kyber: get_version.rs
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

use std::mem;
use std::ptr;
use std::time::Duration;

use log::{info, warn};
use windows_sys::Win32::{Foundation::*, System::IO::*};

use crate::platform::error::{check_win32_error, last_win32_error};
use crate::platform::Event;
use crate::platform::IOCTL_KYBER_IDD_GET_VERSION;
use crate::{Error, Result};

#[repr(C)]
struct Version {
    major: u32,
    minor: u32,
}

pub(crate) fn send_get_version(hdevice: isize, timeout: Duration) -> Result<(u32, u32)> {
    let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };

    let event = Event::new()?;
    overlapped.hEvent = event.handle();

    let mut transferred = 0;

    let mut version = Version { major: 0, minor: 0 };

    let ret = unsafe {
        DeviceIoControl(
            hdevice,
            IOCTL_KYBER_IDD_GET_VERSION,
            ptr::null_mut(),
            0,
            &mut version as *mut _ as *mut _,
            mem::size_of::<Version>() as _,
            &mut transferred,
            &mut overlapped,
        )
    };
    let win32_error = unsafe { GetLastError() };
    if ret == FALSE && win32_error != ERROR_IO_PENDING {
        warn!("DeviceIoControl(GetVersion) failed: 0x{win32_error:X}");
        check_win32_error!(DeviceIoControl, ret == TRUE)?;
    }

    let ret = unsafe {
        GetOverlappedResultEx(
            hdevice,
            &overlapped,
            &mut transferred,
            timeout.as_millis() as _,
            FALSE,
        )
    };
    let win32_error = unsafe { GetLastError() };
    if ret == 0 {
        if win32_error == ERROR_NOT_READY {
            info!("DeviceIoControl(GetVersion): Not ready");
            return Err(Error::DriverNotReady);
        }

        warn!("DeviceIoControl(GetVersion) result failed: 0x{win32_error:X}");
        check_win32_error!(DeviceIoControl, ret == TRUE)?;
    }

    if transferred as usize != mem::size_of::<Version>() {
        warn!("DeviceIoControl(GetVersion) return an unexpected buffer len");
        return Err(Error::InvalidReadSize);
    }

    Ok((version.major, version.minor))
}
