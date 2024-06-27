use log::warn;

use crate::platform::player::FrameSink;
use crate::{MessageProxy, Result};

use super::demuxer_worker::{DemuxerWorker, DemuxerWorkerHandle};
use super::worklet::AudioWorklet;
use super::{CHANNELS, SAMPLE_RATE, SAMPLE_SIZE};

use kyaudioreg::memory::AudioRegulatorSharedMemory;

pub struct AudioPlayer {
    demuxer: DemuxerWorker,
}

impl AudioPlayer {
    pub(crate) const AUDIO_BUFFER_ALLOC_SIZE: u32 = 2 * SAMPLE_RATE;
    pub(crate) const AUDIO_TARGET_BUFFERING: u32 = 50 * 48;

    pub async fn create(message_proxy: MessageProxy) -> Result<(Self, AudioPlayerHandle)> {
        let alloc_size = Self::AUDIO_BUFFER_ALLOC_SIZE;
        let sample_size = SAMPLE_SIZE * CHANNELS;

        let sab = AudioRegulatorSharedMemory::create_sab(alloc_size, sample_size);

        let (demuxer, demuxer_handle) = DemuxerWorker::create(message_proxy)?;
        demuxer.start(sab.clone())?;

        let audio_worklet = AudioWorklet::start(sab).await?;

        let player = Self { demuxer };
        let handle = AudioPlayerHandle {
            audio_worklet: Some(audio_worklet),
            demuxer_handle,
        };

        Ok((player, handle))
    }
}

impl FrameSink for AudioPlayer {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()> {
        self.demuxer.send_frame(frame)?;

        Ok(())
    }
}

// Used to stop the player without the full player instance
pub(crate) struct AudioPlayerHandle {
    audio_worklet: Option<AudioWorklet>,
    demuxer_handle: DemuxerWorkerHandle,
}

impl AudioPlayerHandle {
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(audio_worklet) = self.audio_worklet.take() {
            let ret = audio_worklet.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop audio worklet: {err:?}");
            }
        } else {
            warn!("No AudioWorklet to stop");
        }

        let ret = self.demuxer_handle.stop().await;
        if let Err(err) = ret {
            warn!("Failed to stop audio demuxer: {err:?}");
        }

        Ok(())
    }
}
