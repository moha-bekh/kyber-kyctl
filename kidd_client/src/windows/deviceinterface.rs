// Project Kyber: deviceinterface.rs
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

use std::ffi::OsString;
use std::mem;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::ptr;
use std::slice;
use std::thread;
use std::time::Duration;

use core::ffi::c_void;
use log::{debug, info, warn};
use windows_sys::{
    core::*,
    Win32::{Devices::DeviceAndDriverInstallation::*, Foundation::*, Storage::FileSystem::*},
};

use crate::platform::error::{check_win32_error, last_win32_error};
use crate::platform::requests;
use crate::platform::{DriverHandle, Event};
use crate::{Error, MonitorMode, Result};

/// Default timeout for every DeviceIoControl() call
const IOCTL_TIMEOUT: Duration = Duration::from_secs(5);

/// Wrapper around CM_Register_Notification() to be notified when
/// a Device Interface has been detected.
///
/// Only the creation is notified, the Device interface path isn't
/// available at this point.
pub(crate) struct DeviceInterfaceListener<'a> {
    ctx: isize,
    _event: &'a Event,
}

impl<'a> DeviceInterfaceListener<'a> {
    pub(crate) fn new(guid: &GUID, event: &'a Event) -> Result<DeviceInterfaceListener<'a>> {
        // Configure filter
        let mut filter: CM_NOTIFY_FILTER = unsafe { mem::zeroed() };

        filter.cbSize = mem::size_of::<CM_NOTIFY_FILTER>() as _;
        filter.Flags = 0;
        filter.FilterType = CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE;
        filter.Reserved = 0;

        filter.u = CM_NOTIFY_FILTER_0 {
            DeviceInterface: CM_NOTIFY_FILTER_0_2 { ClassGuid: *guid },
        };

        // Register notifications
        let mut ctx = 0;

        let ret = unsafe {
            CM_Register_Notification(
                &filter,
                event as *const _ as *const _,
                Some(Self::notification_cb),
                &mut ctx,
            )
        };
        if ret != CR_SUCCESS {
            warn!("CM_Register_Notification() failed: 0x{ret:X}");
            return Err(Error::Win32CallRejected);
        }

        Ok(Self { ctx, _event: event })
    }

    unsafe extern "system" fn notification_cb(
        _hnotify: HCMNOTIFICATION,
        userdata: *const c_void,
        action: CM_NOTIFY_ACTION,
        _eventdata: *const CM_NOTIFY_EVENT_DATA,
        _eventdatasize: u32,
    ) -> u32 {
        let event = &*(userdata as *const Event);

        match action {
            CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL => {
                info!("Device interface arrival");

                let ret = event.set();
                if let Err(err) = ret {
                    warn!("Failed to notify device interface arrival: {err:?}");
                }
            }
            CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL => {
                info!("Device interface removal");
            }
            _ => {
                debug!("device_notification_cb({action:X})");
            }
        }

        0
    }
}

impl Drop for DeviceInterfaceListener<'_> {
    fn drop(&mut self) {
        let ret = unsafe { CM_Unregister_Notification(self.ctx) };
        if ret != CR_SUCCESS {
            warn!("CM_Unregister_Notification() failed: 0x{ret:X}");
        }
    }
}

/// Wrapper around SetupDiEnumDeviceInterfaces() to get the DevicePath of the
/// Device interface of the IDD.
///
/// It allows to query existing Device interfaces. DeviceInterfaceListener should
/// be used to make sure that the Device interface exists after probing the IDD
/// with DriverHandler.
pub(crate) struct DeviceInfoFetcher {
    handle: isize,
    interface_data: SP_DEVICE_INTERFACE_DATA,
}

impl DeviceInfoFetcher {
    pub(crate) fn new(guid: &GUID) -> Result<Self> {
        let handle = unsafe {
            SetupDiGetClassDevsW(guid, ptr::null(), 0, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)
        };
        check_win32_error!(SetupDiGetClassDevsW, handle != INVALID_HANDLE_VALUE)?;

        let mut interface_data: SP_DEVICE_INTERFACE_DATA = unsafe { mem::zeroed() };
        interface_data.cbSize = mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as _;

        let ret = unsafe {
            SetupDiEnumDeviceInterfaces(
                handle,
                ptr::null(),
                guid,
                0, // There should be only one interface found
                &mut interface_data,
            )
        };
        check_win32_error!(SetupDiEnumDeviceInterfaces, ret == TRUE)?;

        Ok(Self {
            handle,
            interface_data,
        })
    }

    pub(crate) fn get_path(&self) -> Result<OsString> {
        // Query DeviceInterfaceDetail size.
        // Its size is variable because it contains the device path.
        let mut required_size = 0;

        let ret = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                self.handle,
                &self.interface_data,
                ptr::null_mut(),
                0,
                &mut required_size,
                ptr::null_mut(),
            )
        };
        let win32_error = unsafe { GetLastError() };
        if ret == FALSE && win32_error != ERROR_INSUFFICIENT_BUFFER {
            check_win32_error!(SetupDiGetDeviceInterfaceDetail, ret == TRUE)?;
        }

        // Allocate the buffer that will store the DeviceInterfaceDetail.
        // Then cast it into a *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W to make
        // it usable by Win32 API.
        let interface_data_details_size = required_size;
        let mut interface_data_details_buf = vec![0u8; interface_data_details_size as usize];

        let interface_data_details = unsafe {
            let p: *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W =
                mem::transmute(interface_data_details_buf.as_mut_ptr());
            (*p).cbSize = mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as _;

            p
        };

        // Do get DeviceInterfaceDetail
        let ret = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                self.handle,
                &self.interface_data,
                interface_data_details,
                interface_data_details_size,
                &mut required_size,
                ptr::null_mut(),
            )
        };
        check_win32_error!(SetupDiGetDeviceInterfaceDetail, ret == TRUE)?;

        // Extract DevicePath fromDeviceInterfaceDetail
        let path_len = required_size as usize - mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        let wpath = unsafe {
            slice::from_raw_parts(&(*interface_data_details).DevicePath as *const _, path_len)
        };

        Ok(OsString::from_wide(wpath))
    }
}

impl Drop for DeviceInfoFetcher {
    fn drop(&mut self) {
        unsafe { SetupDiDestroyDeviceInfoList(self.handle) };
    }
}

pub struct DeviceInterface {
    _driver_handle: DriverHandle,
    itf_handle: isize,
}

impl DeviceInterface {
    pub(crate) fn new(driver_handle: DriverHandle, path: &OsString) -> Result<Self> {
        let device_path: Vec<u16> = path.encode_wide().collect();

        let handle = unsafe {
            CreateFileW(
                device_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                0,
            )
        };
        check_win32_error!(CreateFileW, handle != INVALID_HANDLE_VALUE)?;

        Ok(Self {
            _driver_handle: driver_handle,
            itf_handle: handle,
        })
    }

    pub fn get_version(&self) -> Result<(u32, u32)> {
        // Global timeout: 1 second
        const TRY_DELAY: Duration = Duration::from_millis(100);
        const TRY_COUNT: usize = 10;

        for _ in 0..TRY_COUNT {
            match requests::send_get_version(self.itf_handle, IOCTL_TIMEOUT) {
                Ok((major, minor)) => return Ok((major, minor)),
                Err(Error::DriverNotReady) => thread::sleep(TRY_DELAY),
                Err(err) => return Err(err),
            }
        }

        Err(Error::EventTimedout)
    }

    pub fn plug_monitor(&self, modes: &[MonitorMode], prefered_mode: u32) -> Result<()> {
        requests::send_plug_monitor(self.itf_handle, modes, prefered_mode, IOCTL_TIMEOUT)
    }
}

impl Drop for DeviceInterface {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.itf_handle);
        }
    }
}
