use crate::metrics::{Metric, MetricsProducer, VideoMetric, VideoMetricData};
use crate::platform::player::{
    AudioPlayerEvent, AudioPlayerListener, VideoPlayerEvent, VideoPlayerListener, VideoPlayerMetric,
};
use crate::{Message, MessageSender};

use log::warn;

pub(crate) struct VideoMessageForwarder {
    pub message_sender: MessageSender,
    pub metrics: Option<MetricsProducer>,
    pub source_id: u32,
}

impl VideoMessageForwarder {
    fn send(&self, msg: Message) {
        let ret = self.message_sender.send(msg.clone());
        if let Err(err) = ret {
            warn!("Failed to send {msg:?}: {err:?}");
        }
    }

    fn send_metric(&self, metric: Metric) {
        let metrics = self
            .metrics
            .as_ref()
            .expect("Metric received but metrics disabled");
        let ret = metrics.send(metric);
        if let Err(err) = ret {
            warn!("Failed to send {metrics:?}: {err:?}");
        }
    }
}

pub(crate) fn to_metric_data(metric: VideoPlayerMetric) -> VideoMetricData {
    match metric {
        VideoPlayerMetric::FrameReceived => VideoMetricData::FrameReceived,
        VideoPlayerMetric::FrameDecoding => VideoMetricData::FrameDecoding,
        VideoPlayerMetric::FrameDecoded => VideoMetricData::FrameDecoded,
        VideoPlayerMetric::FrameSkipped => VideoMetricData::FrameSkipped,
        VideoPlayerMetric::FrameDisplayed => VideoMetricData::FrameDisplayed,
    }
}

impl VideoPlayerListener for VideoMessageForwarder {
    fn on_event(&self, event: VideoPlayerEvent) {
        match event {
            VideoPlayerEvent::CanvasResized {
                width,
                height,
                frame_coded_width,
                frame_coded_height,
            } => self.send(Message::CanvasResized {
                width,
                height,
                frame_coded_width,
                frame_coded_height,
            }),
            VideoPlayerEvent::Metric { ts, pts, data } => {
                if data == VideoPlayerMetric::FrameDisplayed {
                    // Generate a fake "prepared" event for consistency with the
                    // desktop version using VLC
                    self.send_metric(Metric::Video(VideoMetric {
                        source_id: self.source_id,
                        ts,
                        pts,
                        data: VideoMetricData::FramePrepared,
                    }));
                }
                self.send_metric(Metric::Video(VideoMetric {
                    source_id: self.source_id,
                    ts,
                    pts,
                    data: to_metric_data(data),
                }));
            }
            VideoPlayerEvent::FrameDecoded { pts } => {
                self.send(Message::ImageDecoded { pts });
            }
            VideoPlayerEvent::DecoderError => {
                self.send(Message::VideoDecodeError);
            }
        };
    }
}

pub(crate) struct AudioMessageForwarder {
    pub message_sender: MessageSender,
}

impl AudioMessageForwarder {
    fn send(&self, msg: Message) {
        let ret = self.message_sender.send(msg.clone());
        if let Err(err) = ret {
            warn!("Failed to send {msg:?}: {err:?}");
        }
    }
}

impl AudioPlayerListener for AudioMessageForwarder {
    fn on_event(&self, event: AudioPlayerEvent) {
        match event {
            AudioPlayerEvent::VolumeChanged { volume } => {
                self.send(Message::AudioVolumeChanged { volume });
            }
            AudioPlayerEvent::MuteChanged { mute } => {
                self.send(Message::AudioMuteChanged { mute });
            }
            AudioPlayerEvent::StateChanged { state } => {
                self.send(Message::AudioStateChanged { state });
            }
            AudioPlayerEvent::DecoderError => {
                self.send(Message::AudioDecodeError);
            }
        }
    }
}
