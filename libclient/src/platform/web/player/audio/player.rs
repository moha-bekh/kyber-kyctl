use log::warn;
use std::cell::RefCell;

use crate::platform::player::FrameSink;
use crate::{MessageProxy, Result};

use super::demuxer_worker::DemuxerWorker;
use super::worklet::AudioWorklet;
use super::RingBuffer;
use super::{CHANNELS, SAMPLE_RATE, SAMPLE_SIZE};

pub struct AudioPlayer {
    demuxer: DemuxerWorker,
    audio_worklet: RefCell<Option<AudioWorklet>>,
}

impl AudioPlayer {
    pub async fn new(message_proxy: MessageProxy) -> Result<Self> {
        // Create a 100 ms buffer
        const BUFFER_SIZE: u32 = (SAMPLE_RATE / 10) * CHANNELS * SAMPLE_SIZE;
        let sab = RingBuffer::create_shared_array_buffer(BUFFER_SIZE);

        let demuxer = DemuxerWorker::new(message_proxy)?;
        demuxer.start(sab.clone())?;

        let audio_worklet = AudioWorklet::start(sab, SAMPLE_RATE, CHANNELS).await?;

        Ok(Self {
            demuxer,
            audio_worklet: RefCell::new(Some(audio_worklet)),
        })
    }

    pub async fn stop(&self) -> Result<()> {
        if let Some(audio_worklet) = self.audio_worklet.take() {
            let ret = audio_worklet.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop audio worklet: {err:?}");
            }
        } else {
            warn!("No AudioWorklet to stop");
        }

        let ret = self.demuxer.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop audio demuxer: {err:?}");
        }

        Ok(())
    }
}

impl FrameSink for AudioPlayer {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()> {
        self.demuxer.send_frame(frame)?;

        Ok(())
    }
}
