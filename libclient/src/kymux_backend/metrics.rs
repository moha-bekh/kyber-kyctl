use log::warn;

use crate::metrics::{Metric, MetricsProducer, NetworkStats};
use crate::runtime::{self, Duration};

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
            let metric = Metric::NetworkLocal(NetworkStats {
                rtt_micros: connection_stats
                    .rtt
                    .map(|d| u64::try_from(d.as_micros()).unwrap()),
                packets_lost: connection_stats.packets_lost,
            });
            if let Err(err) = metrics.send(metric) {
                warn!("Metric discarded: {err}");
            }
        }
    });
}
