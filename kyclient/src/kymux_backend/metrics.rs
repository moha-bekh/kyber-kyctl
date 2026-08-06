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

use std::collections::HashMap;

use log::warn;
use serde::Deserialize;
use tokio::sync::mpsc::error::TrySendError;

use kymux::{kyproto, types};

use crate::metrics::{
    Metric, MetricsProducer, NetworkPing, NetworkStats, VideoMetric, VideoMetricData,
};
use crate::runtime::{self, Duration};
use crate::{Error, Result};

/// Send a metric.
///
/// # Returns
///
/// `true` if the channel is still alive, `false` if the channel is closed.
fn send_metric(metrics: &MetricsProducer, metric: Metric) -> bool {
    match metrics.send(metric) {
        Ok(_) => {}
        Err(TrySendError::Closed(_)) => return false,
        Err(TrySendError::Full(_)) => warn!("Metric discarded (channel full)"),
    }

    true
}

pub(crate) fn start_connection_stats_poller(
    connection: &kyproto::Connection,
    metrics: MetricsProducer,
    interval: Duration,
) -> runtime::Task {
    let stats_provider = connection.stats_provider();

    runtime::Task::spawn(async move {
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
            if !send_metric(&metrics, metric) {
                // The metrics consumer has gone away (e.g. after disconnect);
                // stop polling instead of warning on every tick forever.
                break;
            }
        }
    })
}

pub(crate) async fn start_pinger(
    connection: &kyproto::Connection,
    endpoint_id: u16,
    metrics: MetricsProducer,
    interval: Duration,
) -> Result<runtime::Task> {
    let clock_sync = connection.connect_clock_sync_endpoint(endpoint_id)?;
    let protocol = clock_sync
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    let task = runtime::Task::spawn(async move {
        if let Err(err) = run_clock_sync(protocol, metrics, interval).await {
            warn!("Clock sync error: {err:?}");
        }
    });

    Ok(task)
}

async fn run_clock_sync(
    mut protocol: kyproto::ClockSyncClientProtocol,
    metrics: MetricsProducer,
    interval: Duration,
) -> Result<()> {
    let mut interval = runtime::interval(interval);
    loop {
        let result = protocol.sync_average(3).await?;
        let metric = Metric::NetworkPing(NetworkPing {
            offset_micros: result.offset_micros(),
            delay_micros: result.delay_micros(),
        });
        if !send_metric(&metrics, metric) {
            // The metrics consumer has gone away (e.g. after disconnect);
            // stop polling instead of warning on every tick forever.
            break;
        }
        interval.tick().await;
    }

    Ok(())
}

pub(crate) async fn start_remote_metrics_receiver(
    connection: &kyproto::Connection,
    endpoint_id: u16,
    metrics: MetricsProducer,
) -> Result<runtime::Task> {
    let endpoint = connection.connect_metrics_endpoint(endpoint_id)?;
    let protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    let task = runtime::Task::spawn(async move {
        if let Err(err) = run_metrics_receiver(protocol, metrics).await {
            warn!("Remote metrics receiver error: {err:?}");
        }
    });

    Ok(task)
}

async fn run_metrics_receiver(
    mut protocol: types::MetricsClientProtocol,
    metrics: MetricsProducer,
) -> Result<()> {
    while let Some(packet) = protocol.recv.recv().await? {
        let network_stats =
            rmp_serde::from_slice(&packet.payload).map_err(|_| Error::DeserializationFailed)?;
        let metric = Metric::NetworkRemote(network_stats);
        if !send_metric(&metrics, metric) {
            break;
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
    mut protocol: types::MetricsClientProtocol,
    metrics: MetricsProducer,
) -> Result<()> {
    while let Some(packet) = protocol.recv.recv().await? {
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
    connection: &kyproto::Connection,
    source_id: u32,
    endpoint_id: u16,
    metrics: MetricsProducer,
) -> Result<runtime::Task> {
    let endpoint = connection.connect_metrics_endpoint(endpoint_id)?;
    let protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    let task = runtime::Task::spawn(async move {
        if let Err(err) = run_remote_video_metrics_receiver(source_id, protocol, metrics).await {
            warn!("Remote video metrics receiver error: {err:?}");
        }
    });

    Ok(task)
}
