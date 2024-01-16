use async_trait::async_trait;
use log::warn;
use serde::Deserialize;

use crate::platform;
use crate::{ConnectionParams, Display, Message, MessageProxy, Result};

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub(crate) trait WsClient {
    async fn stop(self) -> Result<()>;
}

#[async_trait]
pub(crate) trait WsClientBuilder {
    type Type: WsClient;

    async fn new(conn_params: &ConnectionParams, event_sink: HostEventSink) -> Result<Self::Type>;
}

#[derive(Debug, Deserialize)]
pub(crate) enum HostEvent {
    DisplayList { displays: Vec<Display> },
}

#[derive(Debug, Deserialize)]
pub(crate) enum HostEventFrame {
    Event { event: HostEvent },
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
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        let event_sink = HostEventSink::new(msg_proxy);

        let client = platform::WsClientBuilder::new(conn_params, event_sink).await?;

        Ok(Self { client })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.client.stop().await
    }
}
