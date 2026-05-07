// Project Kyber: input.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

use async_trait::async_trait;
use bytes::{Buf, BufMut};
use log::warn;
use tokio::sync::{mpsc, oneshot};

use kymux::kyproto::ProtocolError;
use kymux::types::{
    InputPacket, InputProtocol, ProtocolEndpointDriver, ProtocolRecv, ProtocolRecvDriver,
    ProtocolSend, ProtocolSendDriver,
};

use crate::{Error, Result};

/// Carries the WebSocket session and receive channel from [`connect`] to
/// [`InputWebSocketEndpoint::ready_boxed`] via a oneshot channel.
pub(super) struct InputConnection {
    /// The actix-ws session to send input packets through
    pub(super) ws_session: actix_ws::Session,
    /// Receives input packets forwarded from the WebSocket
    pub(super) ws_recv: mpsc::Receiver<InputPacket>,
}

/// Implements [`kymux::kyproto::ProtocolEndpoint`] for bidirectional input streams.
///
/// Waits for a WebSocket connection (via [`InputConnection`] delivered over a oneshot),
/// then wraps the session in a [`kymux::kyproto::ProtocolSendDriver`] and the
/// receive channel in a [`kymux::kyproto::ProtocolRecvDriver`] for bidirectional
/// service ↔ client packet forwarding.
pub struct InputWebSocketEndpoint {
    /// Endpoint identifier
    endpoint_id: u16,
    /// Receives the [`InputConnection`] when a WebSocket client connects
    connection_rx: oneshot::Receiver<InputConnection>,
}

impl InputWebSocketEndpoint {
    pub fn new(endpoint_id: u16) -> (Self, oneshot::Sender<InputConnection>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                endpoint_id,
                connection_rx: rx,
            },
            tx,
        )
    }
}

#[async_trait]
impl ProtocolEndpointDriver for InputWebSocketEndpoint {
    type Protocol = InputProtocol;

    fn id(&self) -> u16 {
        self.endpoint_id
    }

    async fn ready_boxed(
        mut self: Box<Self>,
    ) -> std::result::Result<Self::Protocol, ProtocolError> {
        let connection = self
            .connection_rx
            .await
            .map_err(|_| ProtocolError::new("WebSocket session never connected"))?;

        let send = ProtocolSend::new(InputWebSocketSendDriver {
            ws_session: connection.ws_session,
        });
        let recv = ProtocolRecv::new(InputWebSocketRecvDriver {
            rx: connection.ws_recv,
        });

        Ok(InputProtocol { send, recv })
    }
}

/// Implements [`kymux::kyproto::ProtocolSendDriver`] for input packets.
///
/// Serializes [`InputPacket`]s and sends them directly to the WebSocket session.
struct InputWebSocketSendDriver {
    ws_session: actix_ws::Session,
}

#[async_trait]
impl ProtocolSendDriver for InputWebSocketSendDriver {
    type Packet = InputPacket;

    async fn send(&mut self, packet: Self::Packet) -> std::result::Result<(), ProtocolError> {
        let mut buf = bytes::BytesMut::new();
        buf.put_u8(packet.type_);
        buf.put_slice(&packet.payload);

        self.ws_session
            .binary(buf.freeze())
            .await
            .map_err(|_| ProtocolError::new("WebSocket closed"))?;

        Ok(())
    }
}

/// Implements [`kymux::kyproto::ProtocolRecvDriver`] for input packets.
///
/// Receives [`InputPacket`]s from the WebSocket via an mpsc channel.
struct InputWebSocketRecvDriver {
    rx: mpsc::Receiver<InputPacket>,
}

#[async_trait]
impl ProtocolRecvDriver for InputWebSocketRecvDriver {
    type Packet = InputPacket;

    async fn recv(&mut self) -> std::result::Result<Option<Self::Packet>, ProtocolError> {
        Ok(self.rx.recv().await)
    }
}

/// Bridge from the `!Send` [`MessageStream`] to a `Send` mpsc channel.
///
/// [`MessageStream`] is `!Send` so it cannot be stored directly in
/// [`InputWebSocketRecvDriver`] (which requires `Send` via
/// [`kymux::kyproto::ProtocolRecvDriver`]).
async fn forward_ws_to_channel(
    mut ws_rx: actix_ws::MessageStream,
    ws_recv_tx: mpsc::Sender<InputPacket>,
) {
    while let Some(Ok(msg)) = ws_rx.recv().await {
        let actix_ws::Message::Binary(mut msg) = msg else {
            warn!("Unexpected Ws message {msg:?}");
            break;
        };

        if msg.remaining() < 1 {
            warn!("Empty input packet");
            break;
        }

        let type_ = msg.get_u8();
        let packet = InputPacket {
            type_,
            payload: msg,
        };

        if ws_recv_tx.send(packet).await.is_err() {
            warn!("Input channel closed");
            break;
        }
    }
}

/// Bridge a WebSocket connection to an input protocol endpoint.
///
/// Delivers the WebSocket session to the endpoint via the oneshot channel
/// and spawns a task to forward incoming WebSocket messages to the endpoint's
/// receive channel.
pub(super) fn connect(
    connection_tx: oneshot::Sender<InputConnection>,
    ws_session: actix_ws::Session,
    ws_rx: actix_ws::MessageStream,
) -> Result<tokio::task::JoinHandle<()>> {
    let (ws_recv_tx, ws_recv) = mpsc::channel::<InputPacket>(32);

    connection_tx
        .send(InputConnection {
            ws_session,
            ws_recv,
        })
        .map_err(|_| Error::WsComponentNotReady {
            component: "input".into(),
        })?;

    let handle = tokio::task::spawn_local(forward_ws_to_channel(ws_rx, ws_recv_tx));

    Ok(handle)
}
