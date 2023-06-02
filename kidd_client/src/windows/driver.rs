use std::ffi::OsStr;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::time::Duration;

use core::ffi::c_void;
use log::{info, warn};
use windows_sys::{
    core::*,
    Win32::{Devices::Enumeration::Pnp::*, Foundation::*},
};

use crate::platform::Event;
use crate::platform::{DEVICE_DESCRIPTION, DEVICE_ID, PARENT_DEVICE};
use crate::{Error, Result};

fn create_win32_wstr(s: &str) -> Result<Vec<u16>> {
    Ok(OsStr::new(s).encode_wide().chain([0, 0]).collect())
}

/// Wrapper around SwDeviceCreate() to probe the IDD.
/// It waits for SwDeviceCreate() to call the creation callback before
/// returning.
///
/// After return, the driver is probed but the DeviceInterface may not be ready yet.
pub(crate) struct DriverHandle(isize);

impl DriverHandle {
    pub(crate) fn new(timeout: Duration) -> Result<Self> {
        let event = Event::new()?;

        let mut device: isize = 0;

        let device_id = create_win32_wstr(DEVICE_ID)?;
        let device_description = create_win32_wstr(DEVICE_DESCRIPTION)?;
        let parent_device = create_win32_wstr(PARENT_DEVICE)?;

        let mut create_info: SW_DEVICE_CREATE_INFO = unsafe { mem::zeroed() };
        create_info.cbSize = mem::size_of::<SW_DEVICE_CREATE_INFO>() as _;
        create_info.pszInstanceId = device_id.as_ptr();
        create_info.pszzCompatibleIds = device_id.as_ptr();
        create_info.pszzHardwareIds = device_id.as_ptr();
        create_info.pszDeviceDescription = device_description.as_ptr();

        create_info.CapabilityFlags = (SWDeviceCapabilitiesRemovable
            | SWDeviceCapabilitiesSilentInstall
            | SWDeviceCapabilitiesDriverRequired) as _;

        let hr = unsafe {
            SwDeviceCreate(
                device_id.as_ptr(),
                parent_device.as_ptr(),
                &create_info,
                0,
                ptr::null(),
                Some(Self::create_cb),
                &event as *const _ as *const _,
                &mut device,
            )
        };
        if hr != S_OK {
            warn!("SwDeviceCreate() failed: 0x{hr:X}");
            return Err(Error::Win32CallRejected);
        }

        info!("Wait for SwDevice");
        event.wait(timeout)?;

        Ok(Self(device))
    }

    unsafe extern "system" fn create_cb(
        _hswdevice: HSWDEVICE,
        createresult: HRESULT,
        userdata: *const c_void,
        _pszdeviceinstanceid: PCWSTR,
    ) {
        let event = &*(userdata as *const Event);

        info!("Driver device create result: 0x{:X}", createresult);
        if createresult == S_OK {
            let ret = event.set();
            if let Err(err) = ret {
                warn!("Failed to notify device creation: {err:?}");
            }
        }
    }
}

impl Drop for DriverHandle {
    fn drop(&mut self) {
        unsafe { SwDeviceClose(self.0) };
    }
}
