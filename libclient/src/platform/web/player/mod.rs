mod audio;
mod packet;
mod video;

pub(crate) use audio::player::AudioPlayer;
pub(crate) use video::player::VideoPlayer;
pub(crate) use video::{get_video_decoder_config, supported_video_codecs, VideoDecoderConfig};

use crate::Result;

pub(crate) trait FrameSink {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()>;
}
