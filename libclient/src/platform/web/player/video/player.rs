use crate::platform::dom;
use crate::platform::player::FrameSink;
use crate::{MessageProxy, Result};

use super::demuxer_worker::{DemuxerWorker, DemuxerWorkerHandle};
use super::VideoDecoderConfig;

pub struct VideoPlayer {
    worker: DemuxerWorker,
}

impl VideoPlayer {
    pub fn create(
        canvas: &str,
        message_proxy: MessageProxy,
        decoder_config: VideoDecoderConfig,
    ) -> Result<(Self, VideoPlayerHandle)> {
        let canvas = dom::get_canvas(canvas)?;

        let offscreen_canvas = canvas.transfer_control_to_offscreen()?;

        let (worker, worker_handle) = DemuxerWorker::create(message_proxy)?;
        worker.start(offscreen_canvas, decoder_config)?;

        let player = Self { worker };
        let handle = VideoPlayerHandle { worker_handle };

        Ok((player, handle))
    }
}

impl FrameSink for VideoPlayer {
    fn send_frame(&mut self, frame: js_sys::ArrayBuffer) -> Result<()> {
        self.worker.send_frame(frame)?;

        Ok(())
    }
}

// Used to stop the player without the full player instance
pub(crate) struct VideoPlayerHandle {
    worker_handle: DemuxerWorkerHandle,
}

impl VideoPlayerHandle {
    pub async fn stop(&self) -> Result<()> {
        self.worker_handle.stop().await?;

        Ok(())
    }
}
