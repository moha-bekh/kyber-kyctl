use std::mem;
use std::ptr;
use std::time::Duration;

use log::warn;
use windows_sys::Win32::{Foundation::*, System::IO::*};

use crate::platform::error::{check_win32_error, last_win32_error};
use crate::platform::{Event, IOCTL_KYBER_IDD_MONITOR_PLUG};
use crate::{MonitorMode, Result};

use super::{serialize, serialize_array};

#[repr(C)]
struct PlugMonitorRequest {
    mode_count: u32,
    prefered_mode: u32,
}

fn create_payload(request: PlugMonitorRequest, modes: &[MonitorMode]) -> Vec<u8> {
    let raw_request = unsafe { serialize(&request) };
    let raw_modes = unsafe { serialize_array(modes) };

    let payload_size = std::mem::size_of::<PlugMonitorRequest>() + std::mem::size_of_val(modes);
    let mut payload: Vec<u8> = vec![0; payload_size];

    payload[0..raw_request.len()].copy_from_slice(raw_request);
    payload[raw_request.len()..].copy_from_slice(raw_modes);

    payload
}

pub(crate) fn send_plug_monitor(
    hdevice: isize,
    modes: &[MonitorMode],
    prefered_mode: u32,
    timeout: Duration,
) -> Result<()> {
    let mut overlapped: OVERLAPPED = unsafe { mem::zeroed() };

    let event = Event::new()?;
    overlapped.hEvent = event.handle();

    let mut transferred = 0;

    let payload = create_payload(
        PlugMonitorRequest {
            mode_count: modes.len() as _,
            prefered_mode,
        },
        modes,
    );

    let ret = unsafe {
        DeviceIoControl(
            hdevice,
            IOCTL_KYBER_IDD_MONITOR_PLUG,
            payload.as_slice() as *const _ as *const _,
            payload.len() as _,
            ptr::null_mut(),
            0,
            &mut transferred,
            &mut overlapped,
        )
    };
    let win32_error = unsafe { GetLastError() };
    if ret == FALSE && win32_error != ERROR_IO_PENDING {
        warn!("DeviceIoControl(Plug) failed: 0x{win32_error:X}");
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
        warn!("DeviceIoControl(Plug) result failed: 0x{win32_error:X}");
        check_win32_error!(GetOverlappedResultEx, ret == TRUE)?;
    }

    Ok(())
}
