use async_trait::async_trait;
use log::{error, warn};
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

    async fn new(
        url: String,
        conn_params: &ConnectionParams,
        event_sink: HostEventSink,
    ) -> Result<Self::Type>;
}

#[derive(Debug, Deserialize)]
enum HostEvent {
    DisplayList { displays: Vec<Display> },
}

pub(crate) struct HostEventSink {
    msg_proxy: MessageProxy,
}

impl HostEventSink {
    fn new(msg_proxy: MessageProxy) -> Self {
        Self { msg_proxy }
    }

    fn convert_event(event: &str) -> Option<Message> {
        match serde_json::from_str(event) {
            Ok(HostEvent::DisplayList { displays }) => {
                Some(Message::DisplayListUpdated { displays })
            }
            Err(err) => {
                error!("Fail to parse {event}: {err:?}");
                None
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn push(&self, event: &str) {
        if let Some(event) = Self::convert_event(event) {
            let ret = self.msg_proxy.send_async(event).await;
            if let Err(err) = ret {
                warn!("Fail to send HostEvent: {err:?}");
            }
        };
    }

    #[allow(dead_code)]
    pub(crate) fn blocking_push(&self, event: &str) {
        if let Some(event) = Self::convert_event(event) {
            let ret = self.msg_proxy.send(event);
            if let Err(err) = ret {
                warn!("Fail to send HostEvent: {err:?}");
            }
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

        let url = format!(
            "wss://{host}:{port}/ws",
            host = conn_params.host,
            port = conn_params.port
        );

        let client = platform::WsClientBuilder::new(url, conn_params, event_sink).await?;

        Ok(Self { client })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.client.stop().await
    }
}
