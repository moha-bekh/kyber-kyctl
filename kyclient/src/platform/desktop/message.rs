use crate::metrics::{Metric, VideoMetric, VideoMetricData};
use crate::platform::player::{PlayerEvent, PlayerListener, PlayerMetric};
use crate::{Message, MessageSender};

use log::warn;

fn send(message_sender: &MessageSender, msg: Message) {
    let ret = message_sender.send(msg.clone());
    if let Err(err) = ret {
        warn!("Failed to send {msg:?}: {err:?}");
    }
}

pub(crate) struct MessageForwarder {
    pub message_sender: MessageSender,
}

impl MessageForwarder {
    fn send(&self, msg: Message) {
        send(&self.message_sender, msg);
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
                self.send(Message::Metric(Metric::Video(VideoMetric {
                    source_id,
                    ts,
                    pts,
                    data: to_metric_data(data),
                })));
            }
        }
    }
}
