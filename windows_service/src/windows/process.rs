use std::ffi::OsStr;
use std::mem;
use std::os::windows::{ffi::OsStrExt, raw::HANDLE};
use std::ptr;

use log::warn;

use winapi::ctypes::c_void;
use winapi::shared::minwindef::*;
use winapi::um::handleapi::*;
use winapi::um::processthreadsapi::*;
use winapi::um::securitybaseapi::*;
use winapi::um::winbase::*;
use winapi::um::winnt::*;

use super::error::{check_win32_error, last_win32_error, Error, Result};

pub unsafe fn get_subprocess_token() -> Result<HANDLE> {
    let process = GetCurrentProcess();

    let mut process_token: HANDLE = ptr::null_mut();
    let mut b = OpenProcessToken(process, TOKEN_DUPLICATE, &mut process_token);
    check_win32_error!(OpenProcessToken, b == TRUE)?;

    let mut child_token: HANDLE = ptr::null_mut();
    b = DuplicateTokenEx(
        process_token,
        MAXIMUM_ALLOWED,
        ptr::null_mut(),
        SecurityImpersonation,
        TokenPrimary,
        &mut child_token,
    );
    check_win32_error!(DuplicateTokenEx, b == TRUE)?;

    let mut session_id: DWORD = WTSGetActiveConsoleSessionId();
    if session_id == 0xFFFFFFFF {
        warn!("No session attached (WTSGetActiveConsoleSessionId() failed");
        return Err(Error::Rejected);
    }

    b = SetTokenInformation(
        child_token,
        TokenSessionId,
        (&mut session_id as *mut _) as *mut c_void,
        mem::size_of::<DWORD>() as u32,
    );
    check_win32_error!(SetTokenInformation, b == TRUE)?;

    Ok(child_token)
}

pub unsafe fn start_subprocess(path: &str, token: HANDLE) -> Result<HANDLE> {
    let mut si: STARTUPINFOW = mem::zeroed();
    si.cb = mem::size_of::<STARTUPINFOW>() as u32;

    let mut pi: PROCESS_INFORMATION = mem::zeroed();

    let mut wide_path: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let b = CreateProcessAsUserW(
        token,
        ptr::null_mut(),
        wide_path.as_mut_ptr(),
        ptr::null_mut(),
        ptr::null_mut(),
        FALSE,
        NORMAL_PRIORITY_CLASS,
        ptr::null_mut(),
        ptr::null_mut(),
        &mut si,
        &mut pi,
    );
    if b == FALSE {
        warn!("Fail to spawn executable '{}'", path);
        let err = last_win32_error("CreateProcessAsUserW", std::file!(), std::line!());
        return match err {
            Ok(_) => Err(Error::Rejected),
            Err(err) => Err(err),
        };
    }

    CloseHandle(pi.hThread);

    Ok(pi.hProcess)
}
