// Project Kyber: host_events.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::sync::Arc;

use async_trait::async_trait;
use async_tungstenite::{
    tokio::{client_async_tls_with_connector, connect_async, ClientStream},
    tungstenite::Message as tungstenite_Message,
    WebSocketStream,
};
use futures::StreamExt;
use log::{error, info, warn};
use tokio::{net::TcpStream, sync::oneshot, task};

use crate::host_events::{self, HostEventFrame, HostEventSink};
use crate::platform::tls::{self, VerifyMode};
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
        auth_token: &str,
    ) -> Result<Self> {
        let host = conn_params.tls_host.as_ref().unwrap_or(&conn_params.host);
        let port = conn_params.port;
        let url = format!("wss://{host}:{port}/ws");

        // Connect with appropriate TLS verification
        let mut stream = if let Some(verify_mode) = &conn_params.verify_mode {
            Self::connect_with_verify_mode(url, conn_params, verify_mode).await?
        } else {
            Self::connect_without_rootca(url).await?
        };

        info!("Waiting for control plane authentication...");
        let token = format!("{{\"token\":\"{auth_token}\"}}");
        let Ok(_) = stream.send(tungstenite_Message::Text(token.into())).await else {
            return Err(Error::ChannelSend);
        };

        let timer_duration = host_events::timeout_duration(capabilities);
        let Ok(resp) = tokio::time::timeout(timer_duration, stream.next()).await else {
            return Err(Error::HttpQueryTimeout);
        };

        let Some(Ok(tungstenite_Message::Text(status))) = resp else {
            return Err(Error::Rejected);
        };

        let Ok(status) = serde_json::from_str::<&str>(&status) else {
            return Err(Error::Rejected);
        };

        if status != "Welcome" {
            return Err(Error::Rejected);
        }
        info!("Control plane authentified");

        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(
            stream,
            capabilities.clone(),
            event_sink,
            stop_rx,
        ));

        Ok(Self { handle, stop_tx })
    }

    async fn connect_with_verify_mode(
        url: String,
        conn_params: &ConnectionParams,
        verify_mode: &VerifyMode,
    ) -> Result<WebSocketStream<ClientStream<TcpStream>>> {
        let rustls_config = tls::build_client_config(verify_mode).map_err(|err| {
            warn!("Failed to build TLS config: {err:?}");
            Error::TlsVerificationFailed
        })?;

        let stream = TcpStream::connect(conn_params.addr).await?;
        let (stream, _) =
            client_async_tls_with_connector(url, stream, Some(Arc::new(rustls_config).into()))
                .await
                .map_err(|err| {
                    warn!("Failed to open websocket: {err:?}");
                    Error::Rejected
                })?;

        Ok(stream)
    }

    async fn connect_without_rootca(
        url: String,
    ) -> Result<WebSocketStream<ClientStream<TcpStream>>> {
        let (stream, _) = connect_async(url).await.map_err(|err| {
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
                                    reason: Some(close.reason.to_string()),
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

                            let ret = stream.send(tungstenite_Message::Text(pong.into())).await;
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
        auth_token: &str,
    ) -> Result<Self::Type> {
        WsClient::new(conn_params, capabilities, event_sink, auth_token).await
    }
}
