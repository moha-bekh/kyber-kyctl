mod packet;
mod video;

pub(crate) use video::player::VideoPlayer;
pub(crate) use video::{supported_video_codecs, VideoDecoderConfig};

use crate::Result;

pub(crate) trait FrameSink {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()>;
}
