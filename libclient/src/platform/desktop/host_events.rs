use std::sync::Arc;

use async_trait::async_trait;
use async_tungstenite::{
    tokio::{client_async_tls_with_connector, ClientStream},
    tungstenite::Message as tungstenite_Message,
    WebSocketStream,
};
use futures::StreamExt;
use log::warn;
use tokio::{net::TcpStream, sync::oneshot, task};

use crate::host_events::HostEventSink;
use crate::{ConnectionParams, Error, Result};

pub(crate) struct WsClient {
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl WsClient {
    async fn new(conn_params: &ConnectionParams, event_sink: HostEventSink) -> Result<Self> {
        let url = format!(
            "wss://{host}:{port}/ws",
            host = conn_params.tls_host,
            port = conn_params.port
        );

        let stream = Self::connect(url, conn_params).await?;

        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(stream, event_sink, stop_rx));

        Ok(Self { handle, stop_tx })
    }

    async fn connect(
        url: String,
        conn_params: &ConnectionParams,
    ) -> Result<WebSocketStream<ClientStream<TcpStream>>> {
        let stream = TcpStream::connect(conn_params.addr).await?;

        let mut certs_store = rustls::RootCertStore::empty();
        certs_store
            .add(&rustls::Certificate(
                conn_params.tls_config.root_certificate.clone(),
            ))
            .map_err(|err| {
                warn!("Failed to crate RootCertStore: {err:?}");
                Error::Rejected
            })?;

        let rustls_config = Arc::new(
            rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(certs_store)
                .with_no_client_auth(),
        );

        let (stream, _) = client_async_tls_with_connector(url, stream, Some(rustls_config.into()))
            .await
            .map_err(|err| {
                warn!("Failed to open websocket: {err:?}");
                Error::Rejected
            })?;

        Ok(stream)
    }

    async fn task_entry(
        stream: WebSocketStream<ClientStream<TcpStream>>,
        event_sink: HostEventSink,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_stream(stream, event_sink) => {}
        }
    }

    async fn process_stream(
        mut stream: WebSocketStream<ClientStream<TcpStream>>,
        event_sink: HostEventSink,
    ) {
        while let Some(msg) = stream.next().await {
            let Ok(tungstenite_Message::Text(msg)) = msg else {
                continue;
            };

            event_sink.push(&msg).await;
        }
    }
}

#[async_trait]
impl crate::host_events::WsClient for WsClient {
    async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("HostEvent task stop send request failed");
        }

        self.handle.await?;

        Ok(())
    }
}

pub(crate) struct WsClientBuilder;

#[async_trait]
impl crate::host_events::WsClientBuilder for WsClientBuilder {
    type Type = WsClient;

    async fn new(conn_params: &ConnectionParams, event_sink: HostEventSink) -> Result<Self::Type> {
        WsClient::new(conn_params, event_sink).await
    }
}
