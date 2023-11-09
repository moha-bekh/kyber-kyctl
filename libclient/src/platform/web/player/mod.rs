mod packet;

use crate::Result;

pub(crate) trait FrameSink {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()>;
}
