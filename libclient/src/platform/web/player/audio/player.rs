use log::warn;
use std::cell::RefCell;

use crate::platform::player::FrameSink;
use crate::{MessageProxy, Result};

use super::demuxer_worker::DemuxerWorker;
use super::worklet::AudioWorklet;
use super::{CHANNELS, SAMPLE_RATE, SAMPLE_SIZE};

use kyaudioreg::memory::AudioRegulatorSharedMemory;

pub struct AudioPlayer {
    demuxer: DemuxerWorker,
    audio_worklet: RefCell<Option<AudioWorklet>>,
}

impl AudioPlayer {
    pub(crate) const AUDIO_BUFFER_ALLOC_SIZE: u32 = 2 * SAMPLE_RATE;
    pub(crate) const AUDIO_TARGET_BUFFERING: u32 = 50 * 48;

    pub async fn new(message_proxy: MessageProxy) -> Result<Self> {
        let alloc_size = Self::AUDIO_BUFFER_ALLOC_SIZE;
        let sample_size = SAMPLE_SIZE * CHANNELS;

        let sab = AudioRegulatorSharedMemory::create_sab(alloc_size, sample_size);

        let demuxer = DemuxerWorker::new(message_proxy)?;
        demuxer.start(sab.clone())?;

        let audio_worklet = AudioWorklet::start(sab).await?;

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
