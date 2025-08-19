use std::collections::HashMap;

use log::warn;
use serde::Deserialize;

use kymux::{ClockSyncClientProtocol, MetricsPacket, ProtocolEndpoint, ProtocolRecv};

use crate::metrics::{
    Metric, MetricsProducer, NetworkPing, NetworkStats, VideoMetric, VideoMetricData,
};
use crate::runtime::{self, Duration};
use crate::{Error, Result};

pub(crate) async fn start_connection_stats_poller(
    kymux: &kymux::Connection,
    metrics: MetricsProducer,
    interval: Duration,
) {
    let stats_provider = kymux.stats_provider();

    runtime::spawn(async move {
        let mut interval = runtime::interval(interval);
        loop {
            interval.tick().await;
            let connection_stats = stats_provider.connection_stats().await;
            let protocol_stats = stats_provider.protocol_stats();
            let metric = Metric::NetworkLocal(NetworkStats {
                rtt_micros: connection_stats
                    .rtt
                    .map(|d| u64::try_from(d.as_micros()).unwrap()),
                packets_lost: connection_stats.packets_lost,
                dropped_packets: protocol_stats.dropped_packets,
            });
            if let Err(err) = metrics.send(metric) {
                warn!("Metric discarded: {err}");
            }
        }
    });
}

pub(crate) async fn start_pinger(
    kymux: &kymux::Connection,
    endpoint_id: u16,
    metrics: MetricsProducer,
    interval: Duration,
) -> Result<()> {
    let clock_sync = kymux.connect_clock_sync_endpoint(endpoint_id)?;
    let protocol = clock_sync
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    runtime::spawn(async move {
        if let Err(err) = run_clock_sync(protocol, metrics, interval).await {
            warn!("Clock sync error: {err:?}");
        }
    });

    Ok(())
}

async fn run_clock_sync(
    mut protocol: ClockSyncClientProtocol,
    metrics: MetricsProducer,
    interval: Duration,
) -> Result<()> {
    let mut interval = runtime::interval(interval);
    loop {
        let result = protocol
            .sync_average(3)
            .await
            .map_err(|err| Error::Kymux(kymux::Error::KyprotoProtocolError { source: err }))?;
        let metric = Metric::NetworkPing(NetworkPing {
            offset_micros: result.offset_micros(),
            delay_micros: result.delay_micros(),
        });
        if let Err(err) = metrics.send(metric) {
            warn!("Metric discarded: {err}");
        }
        interval.tick().await;
    }
}

pub(crate) async fn start_remote_metrics_receiver(
    kymux: &kymux::Connection,
    endpoint_id: u16,
    metrics: MetricsProducer,
) -> Result<()> {
    let endpoint = kymux.connect_metrics_endpoint(endpoint_id)?;
    let protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    runtime::spawn(async move {
        if let Err(err) = run_metrics_receiver(protocol, metrics).await {
            warn!("Remote metrics receiver error: {err:?}");
        }
    });

    Ok(())
}

async fn run_metrics_receiver(
    mut protocol: ProtocolRecv<MetricsPacket>,
    metrics: MetricsProducer,
) -> Result<()> {
    while let Some(packet) = protocol
        .recv()
        .await
        .map_err(|err| Error::Kymux(err.into()))?
    {
        let network_stats =
            rmp_serde::from_slice(&packet.payload).map_err(|_| Error::DeserializationFailed)?;
        let metric = Metric::NetworkRemote(network_stats);
        if let Err(err) = metrics.send(metric) {
            warn!("Metric discarded: {err}");
        }
    }

    Ok(())
}

// Must match struct Metrics in avserver
#[derive(Debug, Deserialize)]
struct RemoteVideoMetrics {
    key: String,
    value: i64,
}

async fn run_remote_video_metrics_receiver(
    source_id: u32,
    mut protocol: ProtocolRecv<MetricsPacket>,
    metrics: MetricsProducer,
) -> Result<()> {
    while let Some(packet) = protocol
        .recv()
        .await
        .map_err(|err| Error::Kymux(err.into()))?
    {
        let m: Vec<RemoteVideoMetrics> = rmp_serde::from_slice(&packet.payload).unwrap();
        let m: HashMap<&str, i64> = m.iter().map(|m| (m.key.as_str(), m.value)).collect();

        let Some(pts) = m.get("pts").map(|&v| u64::try_from(v).unwrap()) else {
            warn!("Metrics without pts: ignoring");
            continue;
        };

        if let Some(&ts) = m.get("acquired") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameAcquired,
            });
            if metrics.send_force(metric).await.is_err() {
                break;
            }
        };

        if let Some(&ts) = m.get("encoding") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameEncoding,
            });
            if metrics.send_force(metric).await.is_err() {
                break;
            }
        };

        if let Some(&ts) = m.get("encoded") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameEncoded,
            });
            if metrics.send_force(metric).await.is_err() {
                break;
            }
        };

        if let Some(&ts) = m.get("sent") {
            let metric = Metric::Video(VideoMetric {
                source_id,
                pts,
                ts,
                data: VideoMetricData::FrameSent,
            });
            if metrics.send_force(metric).await.is_err() {
                break;
            }
        };
    }

    Ok(())
}

pub(crate) async fn start_remote_video_metrics_receiver(
    kymux: &kymux::Connection,
    source_id: u32,
    endpoint_id: u16,
    metrics: MetricsProducer,
) -> Result<()> {
    let endpoint = kymux.connect_metrics_endpoint(endpoint_id)?;
    let protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    runtime::spawn(async move {
        if let Err(err) = run_remote_video_metrics_receiver(source_id, protocol, metrics).await {
            warn!("Remote video metrics receiver error: {err:?}");
        }
    });

    Ok(())
}
