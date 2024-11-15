#[cfg_attr(feature = "desktop", path = "./desktop.rs")]
#[cfg_attr(feature = "web", path = "./web.rs")]
mod platform;

use std::sync::Arc;

use log::{info, warn};
use serde::Serialize;

use crate::error::Result;
use crate::http::HttpClient;
use crate::{
    BackendDriver, ConnectionParams, DataPlaneCloseReason, Message, MessageProxy,
    StreamingSessionConfig,
};

pub(crate) mod host_service;

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
    pub(crate) streaming_config: &'a StreamingSessionConfig,
    pub(crate) kymux_config: KymuxConfig,
    pub(crate) msg_proxy: MessageProxy,
}

#[cfg(feature = "desktop")]
pub type BoxedKymuxBackend = Box<dyn BackendDriver + Send>;
#[cfg(feature = "web")]
pub type BoxedKymuxBackend = Box<dyn BackendDriver>;

pub(crate) async fn new(config: KymuxBackendConfig<'_>) -> Result<BoxedKymuxBackend> {
    let backend = platform::KymuxBackend::new(config).await?;
    Ok(Box::new(backend))
}

async fn kymux_closed_task(
    connection: Arc<kymux::Connection>,
    msg_proxy: MessageProxy,
) -> Result<()> {
    let ret = connection.closed().await;
    info!("Kymux connection closed: {ret:?}");

    let reason = match ret {
        Ok(()) => DataPlaneCloseReason::Closed,
        Err(_) => DataPlaneCloseReason::Error,
    };

    let ret = msg_proxy
        .send_async(Message::DataPlaneClosed { reason })
        .await;
    if let Err(err) = ret {
        warn!("Fail to send DataPlaneClosed: {err:?}");
    }

    Ok(())
}
