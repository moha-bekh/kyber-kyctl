// Project Kyber: monitors.rs
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

use log::{error, info, warn};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::*;
use windows_sys::core::*;

use crate::windows::config;
use crate::windows::{Result, last_win32_error};

const GUID_DEVCLASS_MONITOR: GUID = GUID {
    data1: 0x4d36e96e,
    data2: 0xe325,
    data3: 0x11ce,
    data4: [0xbf, 0xc1, 0x08, 0x00, 0x2b, 0xe1, 0x03, 0x18],
};

#[allow(clippy::upper_case_acronyms)]
struct HDEVINFO(isize);

impl Drop for HDEVINFO {
    fn drop(&mut self) {
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn is_device_active(dndevinst: u32) -> bool {
    let mut status = 0;
    let mut problem_number = 0;

    let ret = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem_number, dndevinst, 0) };
    if ret != CR_SUCCESS {
        warn!(
            "CM_Get_DevNode_Status() failed: {:?}",
            last_win32_error("CM_Get_DevNode_Status", file!(), line!())
        );
        return false;
    }

    (status & DN_STARTED) == DN_STARTED
}

fn get_monitor_count() -> u32 {
    let device_info_set: isize =
        unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_MONITOR, ptr::null(), 0, DIGCF_PRESENT) };
    if device_info_set == INVALID_HANDLE_VALUE {
        warn!(
            "SetupDiGetClassDevsW() failed: {:?}",
            last_win32_error("SetupDiGetClassDevsW", file!(), line!())
        );
        return 0;
    }

    let device_info_set = HDEVINFO(device_info_set);

    let mut device_info_data: SP_DEVINFO_DATA = unsafe { mem::zeroed() };
    device_info_data.cbSize = mem::size_of::<SP_DEVINFO_DATA>() as _;

    let mut active_monitor_count: u32 = 0;

    for device_id in 0u32.. {
        let ret =
            unsafe { SetupDiEnumDeviceInfo(device_info_set.0, device_id, &mut device_info_data) };
        if ret == FALSE {
            let err = unsafe { GetLastError() };
            if err == ERROR_NO_MORE_ITEMS {
                break;
            }
            warn!(
                "SetupDiEnumDeviceInfo() failed: {:?}",
                last_win32_error("SetupDiEnumDeviceInfo", file!(), line!())
            );
            continue;
        }
        if is_device_active(device_info_data.DevInst) {
            active_monitor_count += 1;
        }
    }

    active_monitor_count
}

fn get_needed_virtual_monitor_count(config: &config::Config) -> u32 {
    use config::KiddPlugStrategy;

    let nb_monitor = get_monitor_count();
    let nb_virtual_monitor = config.kidd.monitor_count;

    match config.kidd.plug_strategy {
        KiddPlugStrategy::WhenNoMonitor => {
            if nb_monitor == 0 {
                nb_virtual_monitor
            } else {
                0
            }
        }
        KiddPlugStrategy::Always => nb_virtual_monitor,
        KiddPlugStrategy::MinimalMonitorCount => nb_virtual_monitor.saturating_sub(nb_monitor),
    }
}

pub(crate) fn start_kidd(config: &config::Config) -> Result<Option<kidd_client::DeviceInterface>> {
    let mut needed_monitor_count: u32 = get_needed_virtual_monitor_count(config);

    if needed_monitor_count == 0 {
        info!("No monitors to plug");
        return Ok(None);
    }

    let mut modes = config::DEFAULT_MONITOR_MODES.to_vec();

    if !config.kidd.extra_modes.is_empty() {
        info!("Adding extra modes to Kidd default configuration");
        let mut extra_modes = config.kidd.extra_modes.iter().map(|m| m.into()).collect();
        info!("Extra modes: {extra_modes:?}");
        modes.append(&mut extra_modes);
    }
    info!("Need to plug monitor, open driver device interface...");
    let device_interface = match kidd_client::Driver::open() {
        Ok(device_interface) => device_interface,
        Err(e) => {
            error!("Failed to open Kidd driver: {:?}", e);
            return Err(e.into());
        }
    };

    let (major, minor) = match device_interface.get_version() {
        Ok(version) => version,
        Err(e) => {
            error!("Failed to get driver version: {:?}", e);
            return Err(e.into());
        }
    };

    if major == 0 && minor == 1 {
        warn!("Driver version 0.1 detected! Limiting to 1 monitor");
        needed_monitor_count = 1;
    }

    info!(
        "Plugging {} monitors (driver version {}.{})",
        needed_monitor_count, major, minor
    );
    for monitor_id in 0..needed_monitor_count {
        match device_interface.plug_monitor(&modes, config::DEFAULT_MONITOR_MODE_IDX) {
            Ok(_) => {
                info!("Virtual monitor {monitor_id} plugged");
            }
            Err(e) => {
                warn!("Failed to plug virtual monitor {monitor_id}");
                return Err(e.into());
            }
        };
    }
    Ok(Some(device_interface))
}
