use crate::platform::dom;
use crate::platform::player::FrameSink;
use crate::{MessageProxy, Result};

use super::demuxer_worker::DemuxerWorker;
use super::VideoDecoderConfig;

pub struct VideoPlayer {
    worker: DemuxerWorker,
}

impl VideoPlayer {
    pub fn new(
        canvas: &str,
        message_proxy: MessageProxy,
        decoder_config: VideoDecoderConfig,
    ) -> Result<Self> {
        let canvas = dom::get_canvas(canvas)?;

        let offscreen_canvas = canvas.transfer_control_to_offscreen()?;

        let worker = DemuxerWorker::new(message_proxy)?;
        worker.start(offscreen_canvas, decoder_config)?;

        Ok(Self { worker })
    }

    pub async fn stop(&self) -> Result<()> {
        self.worker.stop().await?;

        Ok(())
    }
}

impl FrameSink for VideoPlayer {
    fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<()> {
        self.worker.send_frame(frame)?;

        Ok(())
    }
}
