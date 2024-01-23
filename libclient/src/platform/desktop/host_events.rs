use std::sync::Arc;

use async_trait::async_trait;
use async_tungstenite::{
    tokio::{client_async_tls_with_connector, ClientStream},
    tungstenite::Message as tungstenite_Message,
    WebSocketStream,
};
use futures::{SinkExt, StreamExt};
use log::{error, info, warn};
use tokio::{net::TcpStream, sync::oneshot, task};

use crate::host_events::{self, HostEventFrame, HostEventSink};
use crate::{
    ConnectionParams, ControlPlaneCapabilities, ControlPlaneCloseReason, Error, Message, Result,
};

pub(crate) struct WsClient {
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl WsClient {
    async fn new(
        conn_params: &ConnectionParams,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
    ) -> Result<Self> {
        let url = format!(
            "wss://{host}:{port}/ws",
            host = conn_params.tls_host,
            port = conn_params.port
        );

        let stream = Self::connect(url, conn_params).await?;

        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(
            stream,
            capabilities.clone(),
            event_sink,
            stop_rx,
        ));

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
        capabilities: ControlPlaneCapabilities,
        event_sink: HostEventSink,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_stream(stream, capabilities, event_sink) => {}
        }
    }

    async fn process_stream(
        mut stream: WebSocketStream<ClientStream<TcpStream>>,
        capabilities: ControlPlaneCapabilities,
        event_sink: HostEventSink,
    ) {
        let timer_duration = host_events::timeout_duration(&capabilities);
        info!("Setting timeout duration to {timer_duration:?}");

        let timer = tokio::time::sleep(timer_duration);
        tokio::pin!(timer);

        loop {
            tokio::select! {
                biased;
                msg = stream.next() => {
                    let msg = match msg {
                        None => {
                            event_sink.push_msg(Message::ControlPlaneClosed {
                                reason: ControlPlaneCloseReason::Eof,
                            }).await;

                            break;
                        },
                        Some(Err(err)) => {
                            warn!("Control WS receive failed: {err:?}");

                            event_sink.push_msg(Message::ControlPlaneClosed {
                                reason: ControlPlaneCloseReason::Closed {
                                    was_clean: false,
                                    code: None,
                                    reason: None,
                                },
                            }).await;

                            break;
                        }
                        Some(Ok(tungstenite_Message::Text(msg))) => msg,
                        Some(Ok(tungstenite_Message::Close(close))) => {
                            warn!("Control WS closed: {close:?}");

                            let reason = if let Some(close) = close {
                                ControlPlaneCloseReason::Closed {
                                    was_clean: true,
                                    code: Some(close.code.into()),
                                    reason: Some(close.reason.into()),
                                }
                            } else {
                                ControlPlaneCloseReason::Closed {
                                    was_clean: true,
                                    code: None,
                                    reason: None,
                                }
                            };

                            event_sink.push_msg(Message::ControlPlaneClosed {
                                reason
                            }).await;

                            break;
                        }
                        Some(_) => continue,
                    };

                    match serde_json::from_str(&msg) {
                        Ok(HostEventFrame::Ping { id }) => {
                            let pong = HostEventFrame::Pong { id };
                            let pong = match serde_json::to_string(&pong) {
                                Ok(pong) => pong,
                                Err(err) => {
                                    warn!("Fail to convert pong to JSON {pong:?}: {err:?}");
                                    continue;
                                }
                            };

                            timer.as_mut().reset(tokio::time::Instant::now() + timer_duration);

                            let ret = stream.send(tungstenite_Message::Text(pong)).await;
                            if let Err(err) = ret {
                                warn!("Failed to send Pong: {err:?}");
                            }
                        }
                        Ok(HostEventFrame::Pong { id }) => {
                            warn!("Unexpected Pong({id})");
                        }
                        Ok(HostEventFrame::Event { event }) => {
                            event_sink.push(event).await;
                        }
                        Err(err) => {
                            error!("Fail to parse {msg}: {err:?}");
                        }
                    }
                }
                _ = &mut timer => {
                    event_sink.push_msg(Message::ControlPlaneClosed {
                        reason: ControlPlaneCloseReason::Timeout,
                    }).await;

                    break;
                }
            }
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

    async fn new(
        conn_params: &ConnectionParams,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
    ) -> Result<Self::Type> {
        WsClient::new(conn_params, capabilities, event_sink).await
    }
}
