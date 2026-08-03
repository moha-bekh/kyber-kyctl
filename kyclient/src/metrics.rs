// Project Kyber: metrics.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

pub use kyclient_common::metrics::*;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::{SendError, TrySendError};

/// Consumes raw, unprocessed metrics as they are emitted by the streaming pipeline.
///
/// Implementors receive each `Metric` individually and as-is, without any
/// aggregation or clock normalization — suitable for logging raw events or
/// feeding into a `MetricsConsolidator` for further processing.
pub trait RawMetricsConsumer {
    fn on_metric(&mut self, metric: &Metric);
}

/// Consumes consolidated, per-frame metrics once all pipeline stages are accounted for.
///
/// Implementors are notified by a `MetricsConsolidator` when a frame is
/// considered complete, either because all expected events arrived or because
/// the frame fell outside the reassembly window.
pub trait ConsolidatedMetricsConsumer {
    fn on_frame_timestamps(&mut self, frame_timestamps: &FrameTimestamps);
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
pub struct MetricsJSONProducer {
    /// Each inner `Vec<u8>` represents a single metric record encoded as a JSON string in bytes.
    /// The outer `Vec` acts as a write-ahead buffer: records accumulate here
    /// until the caller drains them via `drain`.
    buffer: Vec<Vec<u8>>,
}

impl Default for MetricsJSONProducer {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsJSONProducer {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn drain(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.buffer)
    }

    pub fn push(&mut self, metric: &Metric) -> Vec<Vec<u8>> {
        self.on_metric(metric);
        self.drain()
    }
}

impl RawMetricsConsumer for MetricsJSONProducer {
    fn on_metric(&mut self, metric: &Metric) {
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
                    "log_type": "raw_metric",
                    "type": "video",
                    "event": event,
                    "source_id": source_id,
                    "pts": pts,
                    "ts": ts,
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                self.buffer.push(data);
            }
            Metric::NetworkLocal(network_stats) => {
                let json = json!({
                    "log_type": "raw_metric",
                    "type": "network_local",
                    "rtt_micros": network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
                    "packets_lost": network_stats.packets_lost.map(|v| v as i64).unwrap_or(-1),
                    "dropped_packets": network_stats.dropped_packets.map(|v| v as i64).unwrap_or(-1),
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                self.buffer.push(data);
            }
            Metric::NetworkPing(network_ping) => {
                let json = json!({
                    "log_type": "raw_metric",
                    "type": "network_ping",
                    "offset_micros": network_ping.offset_micros,
                    "delay_micros": network_ping.delay_micros,
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                self.buffer.push(data);
            }
            Metric::NetworkRemote(network_stats) => {
                let json = json!({
                    "log_type": "raw_metric",
                    "type": "network_remote",
                    "rtt_micros": network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
                    "packets_lost": network_stats.packets_lost.map(|v| v as i64).unwrap_or(-1),
                    "dropped_packets": network_stats.dropped_packets.map(|v| v as i64).unwrap_or(-1),
                });
                let data = serde_json::to_vec(&json).expect("Malformed JSON");
                self.buffer.push(data);
            }
        }
    }
}

impl ConsolidatedMetricsConsumer for MetricsJSONProducer {
    fn on_frame_timestamps(&mut self, frame_timestamps: &FrameTimestamps) {
        let mut obj = serde_json::Map::new();
        obj.insert("log_type".to_string(), json!("frame_metric"));
        obj.insert("source_id".to_string(), json!(frame_timestamps.source_id));
        obj.insert("pts".to_string(), json!(frame_timestamps.pts));

        let server_offset = frame_timestamps.server_offset.expect("No server offset");
        obj.insert("server_offset".to_string(), json!(server_offset));

        if let Some(raw_acquired) = frame_timestamps.raw_acquired {
            obj.insert("raw_acquired".to_string(), json!(raw_acquired));
        }
        if let Some(acquired) = frame_timestamps.get_compensated_acquired() {
            obj.insert("acquired".to_string(), json!(acquired));
        }
        if let Some(raw_encoding) = frame_timestamps.raw_encoding {
            obj.insert("raw_encoding".to_string(), json!(raw_encoding));
        }
        if let Some(encoding) = frame_timestamps.get_compensated_encoding() {
            obj.insert("encoding".to_string(), json!(encoding));
        }
        if let Some(raw_encoded) = frame_timestamps.raw_encoded {
            obj.insert("raw_encoded".to_string(), json!(raw_encoded));
        }
        if let Some(encoded) = frame_timestamps.get_compensated_encoded() {
            obj.insert("encoded".to_string(), json!(encoded));
        }
        if let Some(raw_sent) = frame_timestamps.raw_sent {
            obj.insert("raw_sent".to_string(), json!(raw_sent));
        }
        if let Some(sent) = frame_timestamps.get_compensated_sent() {
            obj.insert("sent".to_string(), json!(sent));
        }
        if let Some(received) = frame_timestamps.received {
            obj.insert("received".to_string(), json!(received));
        }
        if let Some(decoding) = frame_timestamps.decoding {
            obj.insert("decoding".to_string(), json!(decoding));
        }
        if let Some(decoded) = frame_timestamps.decoded {
            obj.insert("decoded".to_string(), json!(decoded));
        }
        if let Some(skipped) = frame_timestamps.skipped {
            obj.insert("skipped".to_string(), json!(skipped));
        }
        if let Some(prepared) = frame_timestamps.prepared {
            obj.insert("prepared".to_string(), json!(prepared));
        }
        if let Some(displayed) = frame_timestamps.displayed {
            obj.insert("displayed".to_string(), json!(displayed));
        }
        let json = serde_json::Value::Object(obj);
        let data = serde_json::to_vec(&json).expect("Malformed JSON");
        self.buffer.push(data);
    }
}

/// Reassembles fragmented video metrics into complete, per-frame `FrameTimestamps`.
///
/// A frame is flushed either when all expected stage timestamps are present, or when
/// its PTS falls outside the reassembly window (currently 2 seconds), preventing
/// unbounded memory growth in the presence of dropped metrics.
///
/// `Metric::NetworkPing` events update the server/client offset and backfill any frames
/// already in the buffer.
///
/// # Usage
///
/// Implement `ConsolidatedMetricsConsumer` and pass your consumers at construction time.
/// Feed every incoming `Metric` through `RawMetricsConsumer::on_metric`.
pub struct MetricsConsolidator {
    collector: FrameTimestampsCollector,
    consolidated_consumers: Vec<Box<dyn ConsolidatedMetricsConsumer>>,
}

impl Default for MetricsConsolidator {
    fn default() -> Self {
        Self::new(vec![])
    }
}

impl MetricsConsolidator {
    pub fn new(consolidated_consumers: Vec<Box<dyn ConsolidatedMetricsConsumer>>) -> Self {
        let collector = FrameTimestampsCollector::new(2_000_000); // 2s
        Self {
            collector,
            consolidated_consumers,
        }
    }
}

impl RawMetricsConsumer for MetricsConsolidator {
    fn on_metric(&mut self, metric: &Metric) {
        match metric {
            Metric::Video(video_metric) => {
                self.collector.push(video_metric);
                while let Some(frame_timestamps) = self.collector.recv_frame_timestamps() {
                    for consumer in &mut self.consolidated_consumers {
                        consumer.on_frame_timestamps(&frame_timestamps);
                    }
                }
            }

            Metric::NetworkPing(network_ping) => {
                self.collector
                    .update_clock_offset(network_ping.offset_micros);
            }
            _ => {}
        }
    }
}

/// Raw, per-frame timestamps collected across all stages of the streaming pipeline.
///
/// Each field corresponds to a pipeline stage (server-side: acquired → encoding →
/// encoded → sent; client-side: received → decoding → decoded → prepared/displayed
/// or skipped). Timestamps are stored as microseconds in their original clock domain:
/// server-side values use the server clock, client-side values use the local clock.
///
/// All fields are `Option` because metrics arrive asynchronously and out of order.
/// A frame is considered complete only when all expected fields are populated.
#[derive(Debug, Clone, Default)]
pub struct FrameTimestamps {
    pub source_id: u32,
    pub pts: u64,
    pub server_offset: Option<i64>, // server - client
    pub raw_acquired: Option<i64>,
    pub raw_encoding: Option<i64>,
    pub raw_encoded: Option<i64>,
    pub raw_sent: Option<i64>,
    pub received: Option<i64>,
    pub decoding: Option<i64>,
    pub decoded: Option<i64>,
    pub skipped: Option<i64>,
    pub prepared: Option<i64>,
    pub displayed: Option<i64>,
}

impl FrameTimestamps {
    fn new(source_id: u32, pts: u64) -> Self {
        Self {
            source_id,
            pts,
            ..Default::default()
        }
    }

    fn is_ready(&self) -> bool {
        self.server_offset.is_some()
            && self.raw_acquired.is_some()
            && self.raw_encoding.is_some()
            && self.raw_encoded.is_some()
            && self.raw_sent.is_some()
            && self.received.is_some()
            && self.decoding.is_some()
            && self.decoded.is_some()
            && (self.skipped.is_some() || (self.prepared.is_some() && self.displayed.is_some()))
    }

    fn set_raw_acquired(&mut self, ts: i64) {
        if let Some(prev) = self.raw_acquired {
            log::warn!(
                "Multiple 'acquired' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.raw_acquired = Some(ts);
    }

    fn set_raw_encoding(&mut self, ts: i64) {
        if let Some(prev) = self.raw_encoding {
            log::warn!(
                "Multiple 'encoding' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.raw_encoding = Some(ts);
    }

    fn set_raw_encoded(&mut self, ts: i64) {
        if let Some(prev) = self.raw_encoded {
            log::warn!(
                "Multiple 'encoded' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.raw_encoded = Some(ts);
    }

    fn set_raw_sent(&mut self, ts: i64) {
        if let Some(prev) = self.raw_sent {
            log::warn!(
                "Multiple 'sent' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.raw_sent = Some(ts);
    }

    fn set_received(&mut self, ts: i64) {
        if let Some(prev) = self.received {
            log::warn!(
                "Multiple 'received' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.received = Some(ts);
    }

    fn set_decoding(&mut self, ts: i64) {
        if let Some(prev) = self.decoding {
            log::warn!(
                "Multiple 'decoding' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.decoding = Some(ts);
    }

    fn set_decoded(&mut self, ts: i64) {
        if let Some(prev) = self.decoded {
            log::warn!(
                "Multiple 'decoded' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.decoded = Some(ts);
    }

    fn set_skipped(&mut self, ts: i64) {
        if let Some(prev) = self.skipped {
            log::warn!(
                "Multiple 'skipped' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        if let Some(prev) = self.displayed {
            log::warn!(
                "Incompatible 'skipped' and 'displayed' timestamps for PTS {}: displayed={prev} skipped={ts}",
                self.pts
            );
        }
        self.skipped = Some(ts);
    }

    fn set_prepared(&mut self, ts: i64) {
        if let Some(prev) = self.prepared {
            log::warn!(
                "Multiple 'prepared' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        self.prepared = Some(ts);
    }

    fn set_displayed(&mut self, ts: i64) {
        if let Some(prev) = self.displayed {
            log::warn!(
                "Multiple 'displayed' timestamps for PTS {}: prev={prev} ts={ts}",
                self.pts
            );
        }
        if let Some(prev) = self.skipped {
            log::warn!(
                "Incompatible 'displayed' and 'skipped' timestamps for PTS {}: skipped={prev} displayed={ts}",
                self.pts
            );
        }
        self.displayed = Some(ts);
    }

    fn update(&mut self, metric: &VideoMetric) {
        assert!(metric.pts == self.pts);
        let ts = metric.ts;
        match &metric.data {
            VideoMetricData::FrameAcquired => self.set_raw_acquired(ts),
            VideoMetricData::FrameEncoding => self.set_raw_encoding(ts),
            VideoMetricData::FrameEncoded => self.set_raw_encoded(ts),
            VideoMetricData::FrameSent => self.set_raw_sent(ts),
            VideoMetricData::FrameReceived => self.set_received(ts),
            VideoMetricData::FrameDecoding => self.set_decoding(ts),
            VideoMetricData::FrameDecoded => self.set_decoded(ts),
            VideoMetricData::FrameSkipped => self.set_skipped(ts),
            VideoMetricData::FramePrepared => self.set_prepared(ts),
            VideoMetricData::FrameDisplayed => self.set_displayed(ts),
        }
    }

    pub fn get_compensated_acquired(&self) -> Option<i64> {
        let offset = self.server_offset?;
        self.raw_acquired.map(|ts| ts - offset)
    }

    pub fn get_compensated_encoding(&self) -> Option<i64> {
        let offset = self.server_offset?;
        self.raw_encoding.map(|ts| ts - offset)
    }

    pub fn get_compensated_encoded(&self) -> Option<i64> {
        let offset = self.server_offset?;
        self.raw_encoded.map(|ts| ts - offset)
    }

    pub fn get_compensated_sent(&self) -> Option<i64> {
        let offset = self.server_offset?;
        self.raw_sent.map(|ts| ts - offset)
    }
}

#[derive(Debug)]
struct FrameTimestampsCollector {
    window_us: u64,             // in µs
    server_offset: Option<i64>, // server - client
    sources: HashMap<u32, FrameTimestampsSingleSource>,
}

impl FrameTimestampsCollector {
    fn new(window_us: u64) -> Self {
        Self {
            window_us,
            server_offset: None,
            sources: HashMap::new(),
        }
    }

    fn update_clock_offset(&mut self, server_offset: i64) {
        for source in self.sources.values_mut() {
            source.update_clock_offset(server_offset);
        }

        self.server_offset = Some(server_offset);
    }

    fn push(&mut self, metric: &VideoMetric) {
        let data = self
            .sources
            .entry(metric.source_id)
            .or_insert_with(|| FrameTimestampsSingleSource::new(self.server_offset));

        data.push(metric);
    }

    fn recv_frame_timestamps(&mut self) -> Option<FrameTimestamps> {
        for source in self.sources.values_mut() {
            if let Some(result) = source.recv_frame_timestamps(self.window_us) {
                return Some(result);
            }
        }

        None
    }
}

#[derive(Debug)]
struct FrameTimestampsSingleSource {
    server_offset: Option<i64>, // server - client
    // invariant: sorted by pts
    data: VecDeque<FrameTimestamps>,
    last_pts: Option<u64>,
}

impl FrameTimestampsSingleSource {
    fn new(server_offset: Option<i64>) -> Self {
        Self {
            data: VecDeque::new(),
            server_offset,
            last_pts: None,
        }
    }

    fn update_clock_offset(&mut self, server_offset: i64) {
        if self.server_offset.is_none() {
            for item in &mut self.data {
                assert!(item.server_offset.is_none());
                item.server_offset = Some(server_offset);
            }
        }

        self.server_offset = Some(server_offset);
    }

    fn push(&mut self, metric: &VideoMetric) {
        let pts = metric.pts;

        if self.last_pts.map(|last_pts| pts > last_pts).unwrap_or(true) {
            self.last_pts = Some(pts);
        }

        // Keep the data sorted by pts
        let index = match self.data.binary_search_by_key(&pts, |item| item.pts) {
            Ok(index) => index,
            Err(index) => {
                self.data
                    .insert(index, FrameTimestamps::new(metric.source_id, pts));
                index
            }
        };

        let frame_perfs = &mut self.data[index];
        if frame_perfs.server_offset.is_none() {
            frame_perfs.server_offset = self.server_offset;
        }
        frame_perfs.update(metric);
    }

    fn recv_frame_timestamps(&mut self, window_us: u64) -> Option<FrameTimestamps> {
        let last_pts = self.last_pts?;
        let first = self.data.front()?;

        if first.is_ready() || first.pts < last_pts - std::cmp::min(window_us, last_pts) {
            self.data.pop_front()
        } else {
            None
        }
    }
}
