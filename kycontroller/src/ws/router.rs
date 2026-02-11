// Project Kyber: router.rs
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

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use actix_ws::{MessageStream, Session};
use bytes::{Buf, BufMut};
use log::{info, warn};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Mutex},
    task,
};

use crate::{Error, Result};

use super::Service;

const VIDEO_STREAM_IDX: u16 = 0;
const AUDIO_STREAM_IDX: u16 = 1;
const INPUT_STREAM_IDX: u16 = 2;
const WS_LOCAL_CLIENTS_PORT: u16 = 9090;

pub(super) struct ServiceReadyListener {
    rx_serviceready: mpsc::Receiver<Service>,
    tx_stoplistener: oneshot::Sender<()>,
    listener_joinhandler: task::JoinHandle<()>,
}

impl ServiceReadyListener {
    pub(super) async fn wait_all(mut self, mut pending_services: HashSet<Service>) -> Result<()> {
        while !pending_services.is_empty() {
            let service = self
                .rx_serviceready
                .recv()
                .await
                .ok_or(Error::ChannelClosedRx)?;
            pending_services.remove(&service);
        }

        self.tx_stoplistener
            .send(())
            .map_err(|_| Error::ChannelClosedTx)?;

        let ret = self.listener_joinhandler.await;
        if let Err(err) = ret {
            warn!("Task listener join failed: {err:?}");
        }

        Ok(())
    }
}

struct Task {
    name: &'static str,
    stop_tx: oneshot::Sender<()>,
    handle: task::JoinHandle<()>,
}

impl Task {
    async fn stop(self) {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            info!("Task already stopped");
        }

        let ret = self.handle.await;
        if let Err(err) = ret {
            warn!("Task {} join failed: {err:?}", self.name);
        }
    }
}

struct WsRouterState {
    videoserver: Option<TcpStream>,
    audioserver: Option<TcpStream>,
    inputserver: Option<TcpStream>,

    video_task: Option<Task>,
    audio_task: Option<Task>,
    input_task: Option<Task>,
}

pub(crate) struct WsRouter {
    state: Mutex<WsRouterState>,
}

impl WsRouter {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(WsRouterState {
                videoserver: None,
                audioserver: None,
                inputserver: None,
                video_task: None,
                audio_task: None,
                input_task: None,
            }),
        })
    }

    pub(crate) async fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().await;

        if let Some(video_task) = state.video_task.take() {
            video_task.stop().await;
        }

        if let Some(audio_task) = state.audio_task.take() {
            audio_task.stop().await;
        }

        if let Some(input_task) = state.input_task.take() {
            input_task.stop().await;
        }

        Ok(())
    }

    async fn route_kypackets(mut tcp_rx: OwnedReadHalf, mut ws_tx: Session) -> Result<()> {
        loop {
            const HEADER_SIZE: usize = 12;
            let mut header = vec![0u8; HEADER_SIZE];
            tcp_rx.read_exact(&mut header).await?;

            let is_media_packet = header[0] & 0x80 != 0;
            let mut payload = if is_media_packet {
                let size = u32::from_be_bytes(header[8..12].try_into().unwrap());
                let mut payload = vec![0u8; size as usize];
                tcp_rx.read_exact(&mut payload).await?;

                Some(payload)
            } else {
                None
            };

            let mut ws_payload = header;
            if let Some(payload) = payload.as_mut() {
                ws_payload.append(payload);
            }

            let ret = ws_tx.binary(ws_payload).await;
            if ret.is_err() {
                warn!("Video websocket closed");
                return Ok(());
            }
        }
    }

    pub(super) async fn start_video_routing(
        &self,
        ws_tx: Session,
        _ws_rx: MessageStream,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let mut client = state
            .videoserver
            .take()
            .ok_or_else(|| Error::WsComponentNotReady {
                component: "video".into(),
            })?;
        drop(state);

        let b = [0u8];
        client.write_all(&b).await?;

        let (tcp_rx, _tcp_tx) = client.into_split();

        let (stop_tx, stop_rx) = oneshot::channel();
        let joinhandle = task::spawn_local(async move {
            tokio::select! {
                _ = stop_rx => {
                    info!("Video routing stop requested");
                }
                _ = Self::route_kypackets(tcp_rx, ws_tx) => {
                }
            }
        });

        let mut state = self.state.lock().await;
        state.video_task = Some(Task {
            name: "video",
            handle: joinhandle,
            stop_tx,
        });

        Ok(())
    }

    pub(super) async fn start_audio_routing(
        &self,
        ws_tx: Session,
        _ws_rx: MessageStream,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let mut client = state
            .audioserver
            .take()
            .ok_or_else(|| Error::WsComponentNotReady {
                component: "audio".into(),
            })?;
        drop(state);

        let b = [0u8];
        client.write_all(&b).await?;

        let (tcp_rx, _tcp_tx) = client.into_split();

        let (stop_tx, stop_rx) = oneshot::channel();
        let joinhandle = task::spawn_local(async move {
            tokio::select! {
                _ = stop_rx => {
                    info!("Audio routing stop requested");
                }
                _ = Self::route_kypackets(tcp_rx, ws_tx) => {
                }
            }
        });

        let mut state = self.state.lock().await;
        state.audio_task = Some(Task {
            name: "audio",
            handle: joinhandle,
            stop_tx,
        });

        Ok(())
    }

    async fn route_inputs_rx(mut ws_rx: MessageStream, mut tcp_tx: OwnedWriteHalf) -> Result<()> {
        while let Some(Ok(msg)) = ws_rx.recv().await {
            let actix_ws::Message::Binary(mut msg) = msg else {
                warn!("Unexpected Ws message {msg:?}");
                break;
            };

            let type_ = msg.get_u8();
            let payload = &msg[..];

            tcp_tx.write_u8(type_).await?;
            tcp_tx.write_u16(payload.len() as _).await?;
            tcp_tx.write_all(payload).await?;
        }

        Ok(())
    }

    async fn route_inputs_tx(mut tcp_rx: OwnedReadHalf, mut ws_tx: Session) -> Result<()> {
        const LIBKYNPUT_HEADER_SIZE: usize = 3; // Type(u8) + Len(u16)

        loop {
            // Read and unpack header
            let mut raw_header = [0; LIBKYNPUT_HEADER_SIZE];
            tcp_rx.read_exact(&mut raw_header).await?;

            let mut header = bytes::BytesMut::from(&raw_header[..]);
            let type_ = header.get_u8();
            let size = header.get_u16();

            // Read raw packet
            let mut raw_packet: Vec<u8> = vec![0; size as usize];
            tcp_rx.read_exact(&mut raw_packet[..]).await?;

            // Send packet
            let mut ws_pkt = bytes::BytesMut::new();
            ws_pkt.put_u8(type_);
            ws_pkt.put_slice(&raw_packet);

            let ret = ws_tx.binary(ws_pkt).await;
            if ret.is_err() {
                warn!("Input websocket closed");
                return Ok(());
            }
        }
    }

    pub(super) async fn start_input_routing(
        &self,
        ws_tx: Session,
        ws_rx: MessageStream,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let mut client = state
            .inputserver
            .take()
            .ok_or_else(|| Error::WsComponentNotReady {
                component: "input".into(),
            })?;
        drop(state);

        let b = [0u8];
        client.write_all(&b).await?;

        let (tcp_rx, tcp_tx) = client.into_split();

        let (stop_tx, stop_rx) = oneshot::channel();
        let joinhandle = task::spawn_local(async move {
            tokio::select! {
                _ = stop_rx => {
                    info!("Input routing stop requested");
                }
                _ = Self::route_inputs_rx(ws_rx, tcp_tx) => {
                }
                _ = Self::route_inputs_tx(tcp_rx, ws_tx) => {
                }
            }
        });

        let mut state = self.state.lock().await;
        state.input_task = Some(Task {
            name: "inputs",
            handle: joinhandle,
            stop_tx,
        });

        Ok(())
    }

    pub(super) async fn listen_for_clients(self: &Arc<Self>) -> Result<ServiceReadyListener> {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), WS_LOCAL_CLIENTS_PORT);
        let listener = TcpListener::bind(&addr).await?;

        let this = self.clone();

        let (tx_serviceready, rx_serviceready) = mpsc::channel(32);
        let (tx_stoplistener, rx_stoplistener) = oneshot::channel();

        let joinhandler = task::spawn(async move {
            tokio::select! {
                _ = rx_stoplistener => {
                    info!("Client listener stop requested");
                }
                ret = this.listen_task(listener, tx_serviceready) => {
                    if let Err(err) = ret {
                        warn!("Listen task ended with error: {err:?}");
                    }
                }
            }
        });

        Ok(ServiceReadyListener {
            rx_serviceready,
            tx_stoplistener,
            listener_joinhandler: joinhandler,
        })
    }

    async fn notify_ready(serviceready_tx: &mpsc::Sender<Service>, service: Service) {
        let ret = serviceready_tx.send(service.clone()).await;
        if let Err(err) = ret {
            warn!("Failed to notify that service {service:?} is ready: {err:?}");
        }
    }

    async fn listen_task(
        self: Arc<Self>,
        listener: TcpListener,
        serviceready_tx: mpsc::Sender<Service>,
    ) -> Result<()> {
        loop {
            let (mut client, _addr) = listener.accept().await?;

            // Get Handshake
            let mut b = [0u8; 2];
            client.read_exact(&mut b).await?;

            let endpoint_id = u16::from_be_bytes(b);
            match endpoint_id {
                VIDEO_STREAM_IDX => {
                    info!("Got connection from Video service");

                    let mut state = self.state.lock().await;
                    state.videoserver = Some(client);

                    Self::notify_ready(&serviceready_tx, Service::Video).await;
                }
                AUDIO_STREAM_IDX => {
                    info!("Got connection from Audio service");

                    let mut state = self.state.lock().await;
                    state.audioserver = Some(client);

                    Self::notify_ready(&serviceready_tx, Service::Audio).await;
                }
                INPUT_STREAM_IDX => {
                    info!("Got connection from Input service");

                    let mut state = self.state.lock().await;
                    state.inputserver = Some(client);

                    Self::notify_ready(&serviceready_tx, Service::Input).await;
                }
                _ => {
                    return Err(Error::WsInvalidEndpoint { endpoint_id });
                }
            }
        }
    }
}
