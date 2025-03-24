use crate::error::Result;
use crate::metrics::Metric;

pub struct MetricsProcessor;

impl MetricsProcessor {
    pub async fn new() -> Result<Self> {
        Ok(Self)
    }

    pub async fn on_metric(&mut self, metric: Metric) {
        log::info!("metric: {metric:?}");
    }
}
