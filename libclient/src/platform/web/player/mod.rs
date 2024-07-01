mod audio;
pub mod packet;
mod video;

pub(crate) use audio::player::{AudioPlayer, AudioPlayerHandle};
pub(crate) use video::player::{VideoPlayer, VideoPlayerHandle};
pub(crate) use video::{get_video_decoder_config, supported_video_codecs, VideoDecoderConfig};

use packet::Packet;

use crate::Result;

pub(crate) trait PacketSink {
    fn send_packet(&mut self, packet: Packet) -> Result<()>;
}
