#[repr(C)]
#[derive(Clone, Debug)]
pub struct MonitorMode {
    pub width: u32,
    pub height: u32,
    pub vsync: u32,
}
