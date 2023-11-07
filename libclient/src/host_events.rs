use std::sync::Arc;

use async_tungstenite::{
    tokio::{client_async_tls_with_connector, ClientStream},
    tungstenite::Message as tungstenite_Message,
    WebSocketStream,
};
use futures::StreamExt;
use log::{error, info, warn};
use serde::Deserialize;
use tokio::{net::TcpStream, sync::oneshot, task};

use crate::{ConnectionParams, Display, Error, MessageProxy, Result};

#[derive(Debug, Deserialize)]
enum HostEvent {
    DisplayList { displays: Vec<Display> },
}

pub(crate) struct HostEventReceiver {
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl HostEventReceiver {
    pub(crate) async fn new(
        conn_params: &ConnectionParams,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        let url = format!(
            "wss://{host}:{port}/ws",
            host = conn_params.host,
            port = conn_params.port
        );

        let stream = Self::connect(url, conn_params).await?;

        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(stream, msg_proxy, stop_rx));

        Ok(Self { handle, stop_tx })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("HostEvent task stop send request failed");
        }

        self.handle.await?;

        Ok(())
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

        let config = Arc::new(
            rustls::ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(certs_store)
                .with_no_client_auth(),
        );

        let (stream, _) = client_async_tls_with_connector(url, stream, Some(config.into()))
            .await
            .map_err(|err| {
                warn!("Failed to open websocket: {err:?}");
                Error::Rejected
            })?;

        Ok(stream)
    }

    async fn task_entry(
        stream: WebSocketStream<ClientStream<TcpStream>>,
        msg_proxy: MessageProxy,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_stream(stream, msg_proxy) => {}
        }
    }

    async fn process_stream(
        mut stream: WebSocketStream<ClientStream<TcpStream>>,
        msg_proxy: MessageProxy,
    ) {
        while let Some(msg) = stream.next().await {
            let Ok(tungstenite_Message::Text(msg)) = msg else {
                continue;
            };

            match serde_json::from_str(&msg) {
                Ok(HostEvent::DisplayList { displays }) => {
                    info!("Got Display update: {displays:?}");

                    let ret = msg_proxy
                        .send_async(crate::Message::DisplayListUpdated { displays })
                        .await;
                    if let Err(err) = ret {
                        warn!("Fail to send DisplayListUpdated: {err:?}");
                    }
                }
                Err(err) => {
                    error!("Fail to parse {msg}: {err:?}");
                }
            }
        }
    }
}
