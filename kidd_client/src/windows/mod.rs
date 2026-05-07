// Project Kyber: mod.rs
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

mod deviceinterface;
mod driver;
mod error;
mod event;
mod requests;

use std::time::Duration;

use log::info;
use windows_sys::core::*;

use crate::error::Result;
use event::Event;

pub use deviceinterface::DeviceInterface;
use deviceinterface::{DeviceInfoFetcher, DeviceInterfaceListener};
use driver::DriverHandle;

pub(crate) use error::Win32Error;

// SW Device information
const DEVICE_ID: &str = "KyberIdd";
const DEVICE_DESCRIPTION: &str = "Kyber IDD Driver";
const PARENT_DEVICE: &str = "HTREE\\ROOT\\0";

// 87d73ef0-c08d-4e87-a868-c036d20eb49c
const GUID_DEVINTERFACE: GUID = GUID {
    data1: 0x87d73ef0,
    data2: 0xc08d,
    data3: 0x4e87,
    data4: [0xa8, 0x68, 0xc0, 0x36, 0xd2, 0x0e, 0xb4, 0x9c],
};

#[allow(clippy::identity_op)]
const IOCTL_KYBER_IDD_GET_VERSION: u32 =
    (0x30 << 16) | // IOCTL_CHANGER_BASE
    (0x03 << 14) | // FILE_READ_ACCESS | FILE_WRITE_ACCESS
    (0x800 << 2) | // Function ID
    (0x00)
    // METHOD_BUFFERED
;

#[allow(clippy::identity_op)]
#[allow(dead_code)]
const IOCTL_KYBER_IDD_MONITOR_PLUG: u32 =
    (0x30 << 16) | // IOCTL_CHANGER_BASE
    (0x03 << 14) | // FILE_READ_ACCESS | FILE_WRITE_ACCESS
    (0x801 << 2) | // Function ID
    (0x00)
    // METHOD_BUFFERED
;

pub struct Driver;

impl Driver {
    pub fn open() -> Result<DeviceInterface> {
        // Probe driver and wait its DeviceInterface to be detected
        let itf_arrival_event = Event::new()?;
        let device_itf_listener =
            DeviceInterfaceListener::new(&GUID_DEVINTERFACE, &itf_arrival_event)?;

        let driver_handle = DriverHandle::new(Duration::from_secs(5))?;
        itf_arrival_event.wait(Duration::from_secs(5))?;

        drop(device_itf_listener);

        // Open driver device interface
        let device_info = DeviceInfoFetcher::new(&GUID_DEVINTERFACE)?;
        let device_interface_path = device_info.get_path()?;

        let device_interface = DeviceInterface::new(driver_handle, &device_interface_path)?;
        let (major, minor) = device_interface.get_version()?;
        info!("Kidd version: {major}.{minor}");

        Ok(device_interface)
    }
}
