mod get_version;
mod plug_monitor;

use std::slice;

pub(crate) use get_version::send_get_version;
pub(crate) use plug_monitor::send_plug_monitor;

unsafe fn serialize<T>(data: &T) -> &[u8] {
    slice::from_raw_parts(data as *const _ as *const _, std::mem::size_of::<T>())
}

unsafe fn serialize_array<T>(data: &[T]) -> &[u8] {
    slice::from_raw_parts(data as *const _ as *const _, std::mem::size_of_val(data))
}
