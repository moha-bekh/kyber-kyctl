use crate::error::Result;
use crate::metrics::{Metric, MetricsJSONProducer};

use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};

pub struct MetricsProcessor {
    json_producer: MetricsJSONProducer,
    out: BufWriter<File>,
}

impl MetricsProcessor {
    pub async fn new() -> Result<Self> {
        let json_producer = MetricsJSONProducer;
        let file = File::create("metrics.json").await?;
        let out = BufWriter::new(file);
        Ok(Self { json_producer, out })
    }

    pub async fn close(&mut self) {}

    pub async fn on_metric(&mut self, metric: Metric) {
        if let Some(mut data) = self.json_producer.push(&metric) {
            data.push(b'\n');
            if let Err(err) = self.out.write_all(&data).await {
                log::warn!("Could not write metric: {err:?}");
            }
        }
    }
}
