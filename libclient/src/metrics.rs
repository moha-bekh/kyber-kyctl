use tokio::sync::mpsc;
use tokio::sync::mpsc::error::{SendError, TrySendError};

#[derive(Debug, Clone)]
pub enum Metric {
    Video(VideoMetric),
    NetworkLocal(NetworkStats),
    NetworkPing(NetworkPing),
}

#[derive(Debug, Clone)]
pub struct VideoMetric {
    pub pts: u64,
    pub ts: i64,
    pub data: VideoMetricData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoMetricData {
    FrameReceived,
    FrameDecoding,
    FrameDecoded,
    FrameDisplayed,
    FrameSkipped,
}

// like kymux::ConnectionStats, but a struct local and serializable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub rtt_micros: Option<u64>,
    pub packets_lost: Option<u64>,
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
