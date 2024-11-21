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
use super::{Event, Handle};

fn open_process_token() -> Result<Handle> {
    // MSDN
    // * Retrieves a pseudo handle for the current process.
    // * Calling the CloseHandle function with a pseudo handle has no effect
    let process = unsafe { GetCurrentProcess() };

    let mut process_token: HANDLE = 0;
    let b = unsafe { OpenProcessToken(process, TOKEN_DUPLICATE, &mut process_token) };
    check_win32_error!(OpenProcessToken, b == 1)?;

    Ok(Handle::new(process_token))
}

fn get_subprocess_token_for_session(mut session_id: u32) -> Result<Handle> {
    let process_token = open_process_token()?;

    let mut child_token: HANDLE = 0;
    let b = unsafe {
        DuplicateTokenEx(
            process_token.handle(),
            MAXIMUM_ALLOWED,
            ptr::null_mut(),
            SecurityImpersonation,
            TokenPrimary,
            &mut child_token,
        )
    };
    check_win32_error!(DuplicateTokenEx, b == 1)?;
    let child_token = Handle::new(child_token);

    let b = unsafe {
        SetTokenInformation(
            child_token.handle(),
            TokenSessionId,
            (&mut session_id as *mut _) as *const _,
            mem::size_of::<u32>() as u32,
        )
    };
    check_win32_error!(SetTokenInformation, b == 1)?;

    Ok(child_token)
}

struct ProcThreadAttributeList {
    attributes: LPPROC_THREAD_ATTRIBUTE_LIST,
    _buffer: Vec<u8>,
}

impl ProcThreadAttributeList {
    fn new(inherited: &[&Event]) -> Result<Self> {
        const ATTRIBUTES_COUNT: u32 = 1;

        // Allocate the buffer with the required size to store ATTRIBUTES_COUNT attributes
        let mut attributes_size = 0;
        unsafe {
            InitializeProcThreadAttributeList(
                ptr::null_mut(),
                ATTRIBUTES_COUNT,
                0,
                &mut attributes_size,
            )
        };
        if attributes_size == 0 {
            warn!("InitializeProcThreadAttributeList() returned an empty size");
            return Err(Error::Rejected);
        }

        let mut buffer = vec![0; attributes_size];
        let attributes = buffer.as_mut_ptr() as *mut _;

        let b = unsafe {
            InitializeProcThreadAttributeList(attributes, ATTRIBUTES_COUNT, 0, &mut attributes_size)
        };
        check_win32_error!(InitializeProcThreadAttributeList, b != 0)?;

        let attributes = Self {
            attributes,
            _buffer: buffer,
        };

        // Fill the attribute list with the inherited handles
        let events: Vec<HANDLE> = inherited.iter().map(|e| e.handle()).collect();

        let b = unsafe {
            UpdateProcThreadAttribute(
                attributes.as_ptr(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as _,
                events.as_ptr() as *const _,
                events.len() * mem::size_of::<HANDLE>(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        check_win32_error!(UpdateProcThreadAttribute, b != 0)?;

        Ok(attributes)
    }

    fn as_ptr(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.attributes
    }
}

impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.attributes) };
    }
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

#[derive(Debug)]
pub struct Process {
    handle: Handle,
}

impl Process {
    pub fn new(path: &str, session_id: u32, inherited: &[&Event]) -> Result<Self> {
        let token = get_subprocess_token_for_session(session_id)?;

        let mut si: STARTUPINFOEXW = unsafe { mem::zeroed() };
        si.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;

        let mut pi: PROCESS_INFORMATION = unsafe { mem::zeroed() };

        // Create subprocess env
        // Duplicate current process env + export inherited handles
        let mut env: Vec<(String, String)> = env::vars().collect();
        for event in inherited {
            env.push((event.name().into(), format!("{}", event.handle())));
        }

        let env = create_win32_env(env);

        // Create attribute list. It will contain the list of inherited handles
        let attribute_list = ProcThreadAttributeList::new(inherited)?;
        si.lpAttributeList = attribute_list.as_ptr();

        // Spawn process
        let mut wide_path: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let b = unsafe {
            CreateProcessAsUserW(
                token.handle(),
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
            )
        };
        if b == 0 {
            let err = last_win32_error("CreateProcessAsUserW", std::file!(), std::line!());
            let err = match err {
                Ok(_) => Err(Error::Rejected),
                Err(err) => Err(err),
            };

            warn!("Fail to spawn executable '{path}': {err:?}");
            return err;
        }

        unsafe { CloseHandle(pi.hThread) };

        Ok(Self {
            handle: Handle::new(pi.hProcess),
        })
    }

    pub fn handle(&self) -> HANDLE {
        self.handle.handle()
    }

    pub fn kill(mut self) -> Result<()> {
        self.kill_internal();

        Ok(())
    }

    fn kill_internal(&mut self) {
        unsafe { TerminateProcess(self.handle(), 0) };
        unsafe { WaitForSingleObject(self.handle(), 1000) };
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill_internal();
    }
}
