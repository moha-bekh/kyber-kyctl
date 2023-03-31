use std::net::SocketAddr;

use async_std::net::TcpStream;
use async_tungstenite::{
    async_tls::{client_async_tls_with_connector, ClientStream},
    tungstenite::Message as tungstenite_Message,
    WebSocketStream,
};
use futures::StreamExt;
use log::{error, info, warn};
use serde::Deserialize;
use tokio::{sync::oneshot, task};

use crate::tls;
use crate::{Display, Error, MessageProxy, Result};

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
        streamer_addr: SocketAddr,
        tls_config: tls::Config,
        streamer_tls_host: String,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        let stream = Self::connect(streamer_addr, tls_config, streamer_tls_host).await?;

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
        streamer_addr: SocketAddr,
        tls_config: tls::Config,
        streamer_tls_host: String,
    ) -> Result<WebSocketStream<ClientStream<TcpStream>>> {
        let stream = TcpStream::connect(streamer_addr).await?;

        let mut certs_store = rustls::RootCertStore::empty();
        certs_store
            .add(&rustls::Certificate(tls_config.certificate))
            .map_err(|err| {
                warn!("Failed to crate RootCertStore: {err:?}");
                Error::Rejected
            })?;

        let config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(certs_store)
            .with_no_client_auth();

        let ws_url = format!("wss://{streamer_tls_host}/ws");

        let (stream, _) = client_async_tls_with_connector(ws_url, stream, Some(config.into()))
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
