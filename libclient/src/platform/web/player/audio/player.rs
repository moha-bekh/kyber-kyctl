use wasm_bindgen::prelude::*;

use crate::platform::player::FrameSink;
use crate::Result;

use super::demuxer_worker::DemuxerWorker;
use super::RingBuffer;
use super::{CHANNELS, SAMPLE_RATE, SAMPLE_SIZE};

#[wasm_bindgen(raw_module = "./audio_renderer.js")]
extern "C" {
    type AudioRenderer;

    #[wasm_bindgen(constructor)]
    fn new() -> AudioRenderer;

    #[wasm_bindgen(method)]
    async fn init(
        this: &AudioRenderer,
        sample_rate: f32,
        channel_count: u32,
        sab: js_sys::SharedArrayBuffer,
    );
}

pub struct AudioPlayer {
    demuxer: DemuxerWorker,
    _audio_renderer: AudioRenderer,
}

impl AudioPlayer {
    pub async fn new() -> Result<Self> {
        // Create a 100 ms buffer
        const BUFFER_SIZE: u32 = (SAMPLE_RATE / 10) * CHANNELS * SAMPLE_SIZE;
        let sab = RingBuffer::create_shared_array_buffer(BUFFER_SIZE);

        let demuxer = DemuxerWorker::new()?;
        demuxer.start(sab.clone())?;

        let audio_renderer = AudioRenderer::new();
        audio_renderer.init(SAMPLE_RATE as _, CHANNELS, sab).await;

        Ok(Self {
            demuxer,
            _audio_renderer: audio_renderer,
        })
    }
}

impl FrameSink for AudioPlayer {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()> {
        self.demuxer.send_frame(frame)?;

        Ok(())
    }
}
