mod decoder;
mod demuxer_worker;
pub(super) mod player;
mod ringbuffer;

use ringbuffer::RingBuffer;

const CODEC: &str = "opus";
const FORMAT: web_sys::AudioSampleFormat = web_sys::AudioSampleFormat::F32;
const SAMPLE_SIZE: u32 = 4;
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
