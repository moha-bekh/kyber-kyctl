#[cfg_attr(feature = "desktop", path = "./desktop.rs")]
#[cfg_attr(feature = "web", path = "./web.rs")]
mod platform;

use serde::Serialize;

use crate::error::Result;
use crate::http::HttpClient;
use crate::metrics::MetricsProducer;
use crate::{BackendDriver, ConnectionParams, MessageSender, StreamingSessionConfig};

pub(crate) mod host_service;
pub(crate) mod metrics;

#[derive(Clone, Copy, Debug, Serialize)]
pub enum KymuxTransportBackend {
    #[cfg(feature = "desktop")]
    Quic,
    #[cfg(feature = "web")]
    Webtransport,
}

#[derive(Clone, Debug, Serialize)]
pub struct KymuxConfig {
    pub backend: KymuxTransportBackend,
    pub video_protocol: Option<kymux::VideoProtocol>,
    pub audio_protocol: Option<kymux::AudioProtocol>,
}

pub(crate) struct KymuxBackendConfig<'a> {
    pub(crate) conn_params: &'a ConnectionParams,
    pub(crate) http_client: &'a HttpClient,
    pub(crate) streaming_config: StreamingSessionConfig,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) msg_sender: MessageSender,
}

#[cfg(feature = "desktop")]
pub type BoxedKymuxBackend = Box<dyn BackendDriver + Send>;
#[cfg(feature = "web")]
pub type BoxedKymuxBackend = Box<dyn BackendDriver>;

pub(crate) async fn new(
    config: KymuxBackendConfig<'_>,
    metrics: MetricsProducer,
) -> Result<BoxedKymuxBackend> {
    let backend = platform::KymuxBackend::new(config, metrics).await?;
    Ok(Box::new(backend))
}
