use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::platform;
use crate::{ConnectionParams, ControlPlaneCapabilities, Display, Message, MessageProxy, Result};

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub(crate) trait WsClient {
    async fn stop(self) -> Result<()>;
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub(crate) trait WsClientBuilder {
    type Type: WsClient;

    async fn new(
        conn_params: &ConnectionParams,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
    ) -> Result<Self::Type>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum StreamerPipelineEventType {
    Stopped,
    Error,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StreamerPipelineEvent {
    pub type_: StreamerPipelineType,
    pub stage: StreamerPipelineStage,
    pub event: StreamerPipelineEventType,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) enum HostEvent {
    DisplayList { displays: Vec<Display> },
    StreamerPipelineEvent(StreamerPipelineEvent),
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) enum HostEventFrame {
    Event { event: HostEvent },
    Ping { id: u64 },
    Pong { id: u64 },
}

pub(crate) struct HostEventSink {
    msg_proxy: MessageProxy,
}

impl HostEventSink {
    fn new(msg_proxy: MessageProxy) -> Self {
        Self { msg_proxy }
    }

    fn convert_event(event: HostEvent) -> Message {
        match event {
            HostEvent::DisplayList { displays } => Message::DisplayListUpdated { displays },
            HostEvent::StreamerPipelineEvent(event) => Message::StreamerPipelineEvent(event),
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn push_msg(&self, msg: Message) {
        let ret = self.msg_proxy.send_async(msg).await;
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }

    #[allow(dead_code)]
    pub(crate) fn blocking_push_msg(&self, msg: Message) {
        let ret = self.msg_proxy.send(msg);
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn push(&self, event: HostEvent) {
        let ret = self.msg_proxy.send_async(Self::convert_event(event)).await;
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }

    #[allow(dead_code)]
    pub(crate) fn blocking_push(&self, event: HostEvent) {
        let ret = self.msg_proxy.send(Self::convert_event(event));
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }
}

pub(crate) struct HostEventReceiver {
    client: platform::WsClient,
}

impl HostEventReceiver {
    pub(crate) async fn new(
        conn_params: &ConnectionParams,
        capabilities: &ControlPlaneCapabilities,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        let event_sink = HostEventSink::new(msg_proxy);

        let client = platform::WsClientBuilder::new(conn_params, capabilities, event_sink).await?;

        Ok(Self { client })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.client.stop().await
    }
}

pub(crate) fn timeout_duration(capabilities: &ControlPlaneCapabilities) -> Duration {
    Duration::from_secs(capabilities.keepintvl * capabilities.keepcnt)
}
