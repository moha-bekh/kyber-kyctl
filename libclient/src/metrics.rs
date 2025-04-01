use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::{SendError, TrySendError};

#[derive(Debug, Clone)]
pub enum Metric {
    Video(VideoMetric),
    NetworkLocal(NetworkStats),
    NetworkPing(NetworkPing),
    NetworkRemote(NetworkStats),
}

#[derive(Debug, Clone)]
pub struct VideoMetric {
    pub source_id: u32,
    pub pts: u64,
    pub ts: i64,
    pub data: VideoMetricData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoMetricData {
    FrameAcquired,
    FrameEncoding,
    FrameEncoded,
    FrameSent,
    FrameReceived,
    FrameDecoding,
    FrameDecoded,
    FramePrepared,
    FrameDisplayed,
    FrameSkipped,
}

// like kymux::ConnectionStats, but a struct local and serializable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub rtt_micros: Option<u64>,
    pub packets_lost: Option<u64>,
    pub dropped_packets: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPing {
    pub offset_micros: i64,
    pub delay_micros: i64,
}

#[derive(Debug)]
pub struct MetricsCollector {
    tx: mpsc::Sender<Metric>,
    rx: mpsc::Receiver<Metric>,
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsCollector {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(1024);
        Self { tx, rx }
    }

    pub fn producer(&self) -> MetricsProducer {
        let tx = self.tx.clone();
        MetricsProducer { tx }
    }

    pub async fn recv(&mut self) -> Option<Metric> {
        self.rx.recv().await
    }
}

#[derive(Debug, Clone)]
pub struct MetricsProducer {
    tx: mpsc::Sender<Metric>,
}

impl MetricsProducer {
    pub fn send(&self, value: Metric) -> Result<(), TrySendError<Metric>> {
        self.tx.try_send(value)
    }

    #[allow(dead_code)]
    pub async fn send_force(&self, value: Metric) -> Result<(), SendError<Metric>> {
        self.tx.send(value).await
    }
}

#[derive(Debug)]
pub struct MetricsJSONProducer;

impl Default for MetricsJSONProducer {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsJSONProducer {
    pub fn new() -> Self {
        Self
    }

    pub fn push(&mut self, metric: &Metric) -> Option<Vec<u8>> {
        match metric {
            Metric::Video(video_metric) => {
                let VideoMetric {
                    source_id,
                    pts,
                    ts,
                    data,
                } = video_metric;
                let event = match data {
                    VideoMetricData::FrameAcquired => "acquired",
                    VideoMetricData::FrameEncoding => "encoding",
                    VideoMetricData::FrameEncoded => "encoded",
                    VideoMetricData::FrameSent => "sent",
                    VideoMetricData::FrameReceived => "received",
                    VideoMetricData::FrameDecoding => "decoding",
                    VideoMetricData::FrameDecoded => "decoded",
                    VideoMetricData::FrameSkipped => "skipped",
                    VideoMetricData::FramePrepared => "prepared",
                    VideoMetricData::FrameDisplayed => "displayed",
                };
                let json = json!({
                    "type": "video",
                    "event": event,
                    "source_id": source_id,
                    "pts": pts,
                    "ts": ts,
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                Some(data)
            }
            Metric::NetworkLocal(network_stats) => {
                let json = json!({
                    "type": "network_local",
                    "rtt_micros": network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
                    "packets_lost": network_stats.packets_lost.map(|v| v as i64).unwrap_or(-1),
                    "dropped_packets": network_stats.dropped_packets.map(|v| v as i64).unwrap_or(-1),
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                Some(data)
            }
            Metric::NetworkPing(network_ping) => {
                let json = json!({
                    "type": "network_ping",
                    "offset_micros": network_ping.offset_micros,
                    "delay_micros": network_ping.delay_micros,
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                Some(data)
            }
            Metric::NetworkRemote(network_stats) => {
                let json = json!({
                    "type": "network_remote",
                    "rtt_micros": network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
                    "packets_lost": network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
                    "dropped_packets": network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                Some(data)
            }
        }
    }
}
