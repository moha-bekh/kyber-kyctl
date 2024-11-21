use windows_sys::Win32::Foundation::*;

#[derive(Debug)]
pub(crate) struct Handle {
    handle: HANDLE,
}

impl Handle {
    pub(crate) fn new(handle: HANDLE) -> Self {
        Self { handle }
    }

    pub(crate) fn handle(&self) -> HANDLE {
        self.handle
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}
