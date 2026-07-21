use crate::metrics::{Metric, MetricsProducer, VideoMetric, VideoMetricData};
use crate::platform::player::{PlayerEvent, PlayerListener, PlayerMetric};
use crate::{Message, MessageSender};

use log::warn;

pub(crate) struct MessageForwarder {
    pub message_sender: MessageSender,
    pub metrics: Option<MetricsProducer>,
}

impl MessageForwarder {
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

pub(crate) fn to_metric_data(metric: PlayerMetric) -> VideoMetricData {
    match metric {
        PlayerMetric::FrameReceived => VideoMetricData::FrameReceived,
        PlayerMetric::FrameDecoding => VideoMetricData::FrameDecoding,
        PlayerMetric::FrameDecoded => VideoMetricData::FrameDecoded,
        PlayerMetric::FrameSkipped => VideoMetricData::FrameSkipped,
        PlayerMetric::FramePrepared => VideoMetricData::FramePrepared,
        PlayerMetric::FrameDisplayed => VideoMetricData::FrameDisplayed,
    }
}

impl PlayerListener for MessageForwarder {
    fn on_event(&self, event: PlayerEvent) {
        match event {
            PlayerEvent::PlayerStarted => {
                self.send(Message::PlayerStarted);
            }
            PlayerEvent::PlayerStopped => {
                self.send(Message::PlayerStopped);
            }
            PlayerEvent::AudioVolumeChanged { volume } => {
                self.send(Message::AudioVolumeChanged { volume });
            }
            PlayerEvent::AudioMuteChanged { mute } => {
                self.send(Message::AudioMuteChanged { mute });
            }
            PlayerEvent::Metric {
                source_id,
                ts,
                pts,
                data,
            } => {
                self.send_metric(Metric::Video(VideoMetric {
                    source_id,
                    ts,
                    pts,
                    data: to_metric_data(data),
                }));
            }
        }
    }
}
