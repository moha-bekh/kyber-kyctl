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

/// Implements [`ProtocolEndpointDriver`] for audio/video streams in mono mode.
///
/// `ready_boxed` blocks on a oneshot until the WS client connects via
/// `/websocket/video` or `/websocket/audio`; until then, kycom's forwarder
/// stalls and the avservice
/// back-pressures through its TCP send buffer. Once the session arrives, the
/// session is wrapped in [`AVWebSocketSendDriver`] and the forwarder resumes.
///
/// Generic over the protocol type (`P`): use [`VideoServerProtocol`] for video
/// or [`AudioServerProtocol`] for audio.
pub(super) struct AVWebSocketEndpoint<P> {
    endpoint_id: u16,
    connection_rx: oneshot::Receiver<actix_ws::Session>,
    _protocol: PhantomData<P>,
}

impl<P> AVWebSocketEndpoint<P> {
    pub(super) fn new(endpoint_id: u16) -> (Self, oneshot::Sender<actix_ws::Session>) {
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

    async fn ready_boxed(self: Box<Self>) -> std::result::Result<Self::Protocol, ProtocolError> {
        let ws_session = self
            .connection_rx
            .await
            .map_err(|_| ProtocolError::new("WebSocket session never connected"))?;

        Ok(P::from_send(ProtocolSend::new(AVWebSocketSendDriver {
            ws_session,
        })))
    }
}

/// Implements [`kymux::kyproto::ProtocolSendDriver`] for AV packets.
///
/// Serializes [`AVPacket`]s and sends them directly to the WebSocket session.
/// Used both as a kydup output (multi mode, one driver per WS subscriber) and
/// as the send side of [`AVWebSocketEndpoint`] (mono mode, direct kycom →
/// avservice → WS pipe).
pub(super) struct AVWebSocketSendDriver {
    ws_session: actix_ws::Session,
}

impl AVWebSocketSendDriver {
    pub(super) fn new(ws_session: actix_ws::Session) -> Self {
        Self { ws_session }
    }
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
