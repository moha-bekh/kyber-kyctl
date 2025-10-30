use std::mem;
use std::ptr;

use log::{info, warn};

use windows_sys::core::*;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Foundation::*;

use crate::windows::config;
use crate::windows::{last_win32_error, Result};

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

fn system_has_monitors() -> bool {
    let device_info_set: isize =
        unsafe { SetupDiGetClassDevsW(&GUID_DEVCLASS_MONITOR, ptr::null(), 0, DIGCF_PRESENT) };
    if device_info_set == INVALID_HANDLE_VALUE {
        warn!(
            "SetupDiGetClassDevsW() failed: {:?}",
            last_win32_error("SetupDiGetClassDevsW", file!(), line!())
        );
        return false;
    }

    let device_info_set = HDEVINFO(device_info_set);

    let mut device_info_data: SP_DEVINFO_DATA = unsafe { mem::zeroed() };
    device_info_data.cbSize = mem::size_of::<SP_DEVINFO_DATA>() as _;

    let ret = unsafe { SetupDiEnumDeviceInfo(device_info_set.0, 0, &mut device_info_data) };
    if ret == FALSE {
        return false;
    }

    let mut status = 0;
    let mut problem_number = 0;

    let ret = unsafe {
        CM_Get_DevNode_Status(
            &mut status,
            &mut problem_number,
            device_info_data.DevInst,
            0,
        )
    };
    if ret != CR_SUCCESS {
        warn!(
            "CM_Get_DevNode_Status() failed: {:?}",
            last_win32_error("CM_Get_DevNode_Status", file!(), line!())
        );
        return false;
    }

    status & DN_STARTED == DN_STARTED
}

pub(crate) fn must_plug_monitor(config: &config::Config) -> bool {
    use config::KiddPlugStrategy;

    let enabled = config.kidd.enabled;
    let plug_strategy = config.kidd.plug_strategy;

    info!("Kidd enabled: {enabled}");

    if !enabled {
        return false;
    }

    let system_has_monitors = system_has_monitors();
    info!("Kidd monitor plug strategy: {plug_strategy:?}");
    info!("System has at least one monitor plugged: {system_has_monitors}");

    match (plug_strategy, system_has_monitors) {
        (KiddPlugStrategy::WhenNoMonitor, false) => true,
        (KiddPlugStrategy::WhenNoMonitor, true) => false,
        (KiddPlugStrategy::Always, _) => true,
    }
}

pub(crate) fn start_kidd(config: &config::Config) -> Result<Option<kidd_client::DeviceInterface>> {
    info!("System has no monitor. Plug one");

    let mut modes = config::DEFAULT_MONITOR_MODES.to_vec();

    if !config.kidd.extra_modes.is_empty() {
        info!("Adding extra modes to Kidd default configuration");
        let mut extra_modes = config.kidd.extra_modes.iter().map(|m| m.into()).collect();
        info!("Extra modes: {extra_modes:?}");
        modes.append(&mut extra_modes);
    }

    let device_interface = kidd_client::Driver::open()?;
    for _ in 0..config.kidd.monitor_count {
        device_interface.plug_monitor(&modes, config::DEFAULT_MONITOR_MODE_IDX)?;
    }

    Ok(Some(device_interface))
}
