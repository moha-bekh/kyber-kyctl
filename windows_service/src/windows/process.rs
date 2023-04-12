use std::env;
use std::ffi::{CString, OsStr};
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
use super::Event;

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

fn create_win32_env(env: Vec<(String, String)>) -> Vec<u8> {
    env.iter()
        .filter_map(|(k, v)| {
            if let Ok(s) = CString::new(format!("{k}={v}")) {
                Some(s.to_bytes_with_nul().to_vec())
            } else {
                warn!("Failed to generate CString for {k}={v}");
                None
            }
        })
        .flatten()
        // MSDN note: "An ANSI environment block is terminated by two zero bytes:
        // one for the last string, one more to terminate the block".
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn create_attribute_list(
    inherited: &[&Event],
) -> Result<(Vec<u8>, LPPROC_THREAD_ATTRIBUTE_LIST)> {
    const ATTRIBUTES_COUNT: u32 = 1;

    // Allocate the buffer with the required size to store ATTRIBUTES_COUNT attributes
    let mut attributes_size = 0;
    InitializeProcThreadAttributeList(ptr::null_mut(), ATTRIBUTES_COUNT, 0, &mut attributes_size);
    if attributes_size == 0 {
        warn!("InitializeProcThreadAttributeList() returned an empty size");
        return Err(Error::Rejected);
    }

    let mut buffer = vec![0; attributes_size];
    let attributes = buffer.as_mut_ptr() as *mut _;

    // Create a fill the attribute list with the inherited handles
    let b =
        InitializeProcThreadAttributeList(attributes, ATTRIBUTES_COUNT, 0, &mut attributes_size);
    check_win32_error!(InitializeProcThreadAttributeList, b != 0)?;

    let events: Vec<HANDLE> = inherited.iter().map(|e| e.handle()).collect();

    let b = UpdateProcThreadAttribute(
        attributes,
        0,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as _,
        events.as_ptr() as *const _,
        events.len() * mem::size_of::<HANDLE>(),
        ptr::null_mut(),
        ptr::null(),
    );
    if b == 0 {
        DeleteProcThreadAttributeList(attributes);
        check_win32_error!(UpdateProcThreadAttribute, b != 0)?;
    }

    Ok((buffer, attributes))
}

pub unsafe fn start_subprocess(path: &str, token: HANDLE, inherited: &[&Event]) -> Result<HANDLE> {
    let mut si: STARTUPINFOEXW = mem::zeroed();
    si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;

    let mut pi: PROCESS_INFORMATION = mem::zeroed();

    // Create subprocess env
    // Duplicate current process env + export inherited handles
    let mut env: Vec<(String, String)> = env::vars().collect();
    for event in inherited {
        env.push((event.name().into(), format!("{}", event.handle())));
    }

    let env = create_win32_env(env);

    // Create attribute list. It will contain the list of inherited handles
    let (_attributes_buf, attributes_ptr) = create_attribute_list(inherited)?;
    si.lpAttributeList = attributes_ptr;

    // Spawn process
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
        TRUE,
        NORMAL_PRIORITY_CLASS | EXTENDED_STARTUPINFO_PRESENT,
        env.as_ptr() as *const _,
        ptr::null_mut(),
        &si.StartupInfo,
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
    DeleteProcThreadAttributeList(attributes_ptr);

    Ok(pi.hProcess)
}
