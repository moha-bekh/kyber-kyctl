// Project Kyber: av.rs
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

use std::marker::PhantomData;

use async_trait::async_trait;
use bytes::BufMut;
use tokio::sync::oneshot;

use kymux::kyproto::ProtocolError;
use kymux::types::{
    AVPacket, AudioServerProtocol, ProtocolEndpointDriver, ProtocolSend, ProtocolSendDriver,
    VideoServerProtocol,
};

use crate::{Error, Result};

/// Trait for protocol types that wrap a single [`ProtocolSend<AVPacket>`].
trait AVSendProtocol: 'static {
    fn from_send(send: ProtocolSend<AVPacket>) -> Self;
}

impl AVSendProtocol for VideoServerProtocol {
    fn from_send(send: ProtocolSend<AVPacket>) -> Self {
        Self { send }
    }
}

impl AVSendProtocol for AudioServerProtocol {
    fn from_send(send: ProtocolSend<AVPacket>) -> Self {
        Self { send }
    }
}

/// Carries the WebSocket session from [`connect`] to [`AVWebSocketEndpoint::ready_boxed`]
/// via a oneshot channel.
pub(super) struct AVConnection {
    /// The actix-ws session to send AV packets through
    pub(super) ws_session: actix_ws::Session,
}

/// Implements [`ProtocolEndpointDriver`] for audio/video streams.
///
/// Waits for a WebSocket connection (via [`AVConnection`] delivered over a oneshot),
/// then wraps the session in a [`ProtocolSendDriver`] for
/// unidirectional service → client packet forwarding.
///
/// Generic over the protocol type (`P`): use [`VideoServerProtocol`] for video
/// or [`AudioServerProtocol`] for audio.
pub struct AVWebSocketEndpoint<P> {
    /// Endpoint identifier (e.g. 0=video, 1=audio)
    endpoint_id: u16,
    /// Receives the [`AVConnection`] when a WebSocket client connects
    connection_rx: oneshot::Receiver<AVConnection>,
    _protocol: PhantomData<P>,
}

impl<P> AVWebSocketEndpoint<P> {
    pub fn new(endpoint_id: u16) -> (Self, oneshot::Sender<AVConnection>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                endpoint_id,
                connection_rx: rx,
                _protocol: PhantomData,
            },
            tx,
        )
    }
}

#[async_trait]
impl<P: AVSendProtocol + Send> ProtocolEndpointDriver for AVWebSocketEndpoint<P> {
    type Protocol = P;

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

        Ok(P::from_send(ProtocolSend::new(AVWebSocketSendDriver {
            ws_session: connection.ws_session,
        })))
    }
}

/// Implements [`kymux::kyproto::ProtocolSendDriver`] for AV packets.
///
/// Serializes [`AVPacket`]s and sends them directly to the WebSocket session.
struct AVWebSocketSendDriver {
    ws_session: actix_ws::Session,
}

#[async_trait]
impl ProtocolSendDriver for AVWebSocketSendDriver {
    type Packet = AVPacket;

    async fn send(&mut self, packet: Self::Packet) -> std::result::Result<(), ProtocolError> {
        let buf = match packet {
            AVPacket::Codec(p) => {
                let header = p.header.serialize();

                let mut buf = bytes::BytesMut::with_capacity(header.len());
                buf.put_slice(&header);

                buf
            }
            AVPacket::Media(p) => {
                let header = p.header.serialize();

                let mut buf = bytes::BytesMut::with_capacity(header.len() + p.payload.len());
                buf.put_slice(&header);
                buf.put_slice(&p.payload);

                buf
            }
            AVPacket::Hole(p) => {
                let header = p.header.serialize();

                let mut buf = bytes::BytesMut::with_capacity(header.len());
                buf.put_slice(&header);

                buf
            }
        };

        self.ws_session
            .binary(buf.freeze())
            .await
            .map_err(|_| ProtocolError::new("WebSocket closed"))?;

        Ok(())
    }
}

/// Bridge a WebSocket connection to an AV protocol endpoint.
///
/// Delivers the WebSocket session to the endpoint via the oneshot channel.
/// The endpoint's [`AVWebSocketSendDriver`] will then send packets directly
/// to the session.
pub(super) fn connect(
    component: &'static str,
    connection_tx: oneshot::Sender<AVConnection>,
    ws_session: actix_ws::Session,
    _ws_rx: actix_ws::MessageStream,
) -> Result<()> {
    connection_tx
        .send(AVConnection { ws_session })
        .map_err(|_| Error::WsComponentNotReady {
            component: component.into(),
        })?;

    Ok(())
}
