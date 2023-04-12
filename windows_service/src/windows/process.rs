use std::ffi::OsStr;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use log::warn;
use windows_sys::Win32::{
    Foundation::*,
    Security::*,
    System::{SystemServices::*, Threading::*},
};

use super::error::{check_win32_error, last_win32_error, Error, Result};

pub unsafe fn get_subprocess_token(mut session_id: u32) -> Result<HANDLE> {
    let process = GetCurrentProcess();

    let mut process_token: HANDLE = 0;
    let mut b = OpenProcessToken(process, TOKEN_DUPLICATE, &mut process_token);
    check_win32_error!(OpenProcessToken, b == 1)?;

    let mut child_token: HANDLE = 0;
    b = DuplicateTokenEx(
        process_token,
        MAXIMUM_ALLOWED,
        ptr::null_mut(),
        SecurityImpersonation,
        TokenPrimary,
        &mut child_token,
    );
    check_win32_error!(DuplicateTokenEx, b == 1)?;

    b = SetTokenInformation(
        child_token,
        TokenSessionId,
        (&mut session_id as *mut _) as *const _,
        mem::size_of::<u32>() as u32,
    );
    check_win32_error!(SetTokenInformation, b == 1)?;

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
        0,
        NORMAL_PRIORITY_CLASS,
        ptr::null_mut(),
        ptr::null_mut(),
        &si,
        &mut pi,
    );
    if b == 0 {
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
