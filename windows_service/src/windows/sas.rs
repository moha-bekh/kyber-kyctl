use windows_sys::Win32::{Foundation::*, Security::Authentication::Identity::SendSAS};

pub fn send() {
    unsafe { SendSAS(FALSE) };
}
