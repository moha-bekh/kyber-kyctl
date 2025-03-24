use super::dom;
use crate::error::Result;
use crate::metrics::{Metric, MetricsJSONProducer};

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

pub struct MetricsProcessor {
    json_producer: MetricsJSONProducer,
    stream: web_sys::FileSystemWritableFileStream,
}

impl MetricsProcessor {
    pub async fn new() -> Result<Self> {
        let json_producer = MetricsJSONProducer;
        let stream = Self::select_file().await?;
        Ok(Self {
            json_producer,
            stream,
        })
    }

    async fn select_file() -> Result<web_sys::FileSystemWritableFileStream> {
        let window = dom::window()?;
        let navigator = window.navigator();

        let storage = navigator.storage();

        let dir = JsFuture::from(storage.get_directory())
            .await?
            .dyn_into::<web_sys::FileSystemDirectoryHandle>()?;

        let options = web_sys::FileSystemGetFileOptions::new();
        options.set_create(true);
        let file = JsFuture::from(dir.get_file_handle_with_options("metrics.json", &options))
            .await?
            .dyn_into::<web_sys::FileSystemFileHandle>()?;

        let stream = JsFuture::from(file.create_writable())
            .await?
            .dyn_into::<web_sys::FileSystemWritableFileStream>()?;

        Ok(stream)
    }

    pub async fn close(&mut self) {
        if let Err(err) = JsFuture::from(self.stream.close()).await {
            log::warn!("Could not close file stream: {err:?}");
        }
    }

    pub async fn on_metric(&mut self, metric: Metric) {
        if let Some(mut data) = self.json_producer.push(&metric) {
            data.push(b'\n');
            if let Err(err) = self.append(&data).await {
                log::warn!("Could not write metric: {err:?}");
            }
        }
    }

    async fn append(&mut self, raw: &[u8]) -> Result<()> {
        let promise = self.stream.write_with_u8_array(&raw)?;
        JsFuture::from(promise).await?;
        Ok(())
    }
}
