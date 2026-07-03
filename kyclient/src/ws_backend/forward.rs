// Project Kyber: forward.rs
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

use crate::{Error, Result};

use std::sync::Arc;

use kymux::{kywebsocket, types};
use kynput::bufferproxy::{BufferProxyReceiver, BufferProxySender};
use kynput::{InputNetworkStreamMsg, InputNetworkStreamObserver};
use kywebplayer as player;
use log::{info, warn};
use tokio::sync::oneshot;

// Will interrupt the receiver on drop
pub struct WebSocketHandle {
    _stop_tx: oneshot::Sender<()>,
}

pub fn forward_video(
    conn: kywebsocket::Connection,
    player: player::VideoPlayer,
) -> WebSocketHandle {
    let endpoint = conn.into_channel().into_video_client_endpoint();

    let (stop_tx, stop_rx) = oneshot::channel();
    wasm_bindgen_futures::spawn_local(video_player_forward_task(endpoint, player, stop_rx));
    WebSocketHandle { _stop_tx: stop_tx }
}

pub fn forward_audio(
    conn: kywebsocket::Connection,
    player: player::AudioPlayer,
) -> WebSocketHandle {
    let endpoint = conn.into_channel().into_audio_client_endpoint();

    let (stop_tx, stop_rx) = oneshot::channel();
    wasm_bindgen_futures::spawn_local(audio_player_forward_task(endpoint, player, stop_rx));
    WebSocketHandle { _stop_tx: stop_tx }
}

pub fn forward_input(
    conn: kywebsocket::Connection,
    kynput_tx: BufferProxySender,
    kynput_rx: BufferProxyReceiver,
    msg_sink: Arc<dyn InputNetworkStreamObserver>,
) -> WebSocketHandle {
    let endpoint = conn.into_channel().into_input_endpoint();

    msg_sink.on_message(InputNetworkStreamMsg::ClientConnected);

    let (stop_tx, stop_rx) = oneshot::channel();
    wasm_bindgen_futures::spawn_local(async move {
        input_forward_task(endpoint, kynput_tx, kynput_rx, stop_rx).await;
        msg_sink.on_message(InputNetworkStreamMsg::ClientDisconnected);
    });
    WebSocketHandle { _stop_tx: stop_tx }
}

async fn video_player_forward_task(
    endpoint: types::VideoClientEndpoint,
    player: player::VideoPlayer,
    stop_rx: oneshot::Receiver<()>,
) {
    match endpoint.ready().await {
        Ok(protocol) => player_forward_task("video", protocol.recv, player, stop_rx).await,
        Err(err) => warn!("Failed to create protocol: {err:?}"),
    }
}

async fn audio_player_forward_task(
    endpoint: types::AudioClientEndpoint,
    player: player::AudioPlayer,
    stop_rx: oneshot::Receiver<()>,
) {
    match endpoint.ready().await {
        Ok(protocol) => player_forward_task("audio", protocol.recv, player, stop_rx).await,
        Err(err) => warn!("Failed to create protocol: {err:?}"),
    }
}

async fn player_forward_task<Player>(
    type_: &str,
    mut recv: types::ProtocolRecv<types::AVPacket>,
    mut player: Player,
    mut stop_rx: oneshot::Receiver<()>,
) where
    Player: player::PacketSink,
{
    loop {
        tokio::select! {
            kypacket = recv.recv() => {
                let Ok(Some(kypacket)) = kypacket else {
                    if let Err(err) = kypacket {
                        warn!("Failed to receive {type_} from Kymux: {err:?}");
                    }
                    return;
                };

                let packet = to_player_packet(kypacket);
                let ret = player.send_packet(packet);
                if let Err(err) = ret {
                    warn!("Failed to send {type_} frame to player: {err:?}");
                }
            }
            _ = &mut stop_rx => return,
        }
    }
}

fn to_player_packet(packet: types::AVPacket) -> player::packet::Packet {
    match packet {
        types::AVPacket::Codec(packet) => {
            player::packet::Packet::Codec(player::packet::CodecPacket {
                codec: packet.header.codec,
                rotation: packet.header.rotation,
            })
        }
        types::AVPacket::Media(packet) => {
            player::packet::Packet::Media(player::packet::MediaPacket {
                header: player::packet::MediaPacketHeader {
                    is_config: packet.header.is_config,
                    is_key: packet.header.is_key,
                    pts: packet.header.pts,
                    size: packet.header.size,
                },
                data: {
                    let buffer = js_sys::Uint8Array::new_with_length(packet.payload.len() as _);
                    buffer.copy_from(&packet.payload);
                    buffer
                },
            })
        }
        types::AVPacket::Hole(packet) => player::packet::Packet::Hole(player::packet::HolePacket {
            missing_audio_samples: packet.header.missing_audio_samples,
        }),
    }
}

async fn input_forward_task(
    endpoint: types::InputEndpoint,
    kynput_tx: BufferProxySender,
    kynput_rx: BufferProxyReceiver,
    mut stop_rx: oneshot::Receiver<()>,
) {
    let protocol = match endpoint.ready().await {
        Ok(protocol) => protocol,
        Err(err) => {
            warn!("Failed to create protocol: {err:?}");
            return;
        }
    };
    tokio::select! {
        ret = input_rx_task(protocol.recv, kynput_tx) => {
            info!("End of Input Rx task. Result: {ret:?}");
        },
        ret = input_tx_task(protocol.send, kynput_rx) => {
            info!("End of Input Tx task. Result: {ret:?}");
        },
        _ = &mut stop_rx => {},
    }
}

async fn input_rx_task(
    mut endpoint_rx: types::ProtocolRecv<types::InputPacket>,
    kynput_tx: BufferProxySender,
) -> Result<()> {
    loop {
        match endpoint_rx.recv().await {
            Ok(Some(pkt)) => {
                use bytes::BufMut;

                let mut packed_pkt = bytes::BytesMut::new();
                packed_pkt.put_u8(pkt.type_);
                packed_pkt.put_slice(&pkt.payload);

                kynput_tx.send(packed_pkt.to_vec()).await?;
            }
            Ok(None) => {
                info!("Input Rx task: EOS");
                break;
            }
            Err(err) => {
                warn!("Input Rx task error: {err:?}");
                break;
            }
        }
    }

    Ok(())
}

async fn input_tx_task(
    mut endpoint_tx: types::ProtocolSend<types::InputPacket>,
    mut kynput_rx: BufferProxyReceiver,
) -> Result<()> {
    while let Ok(buf) = kynput_rx.recv().await {
        endpoint_tx
            .send(types::InputPacket {
                type_: buf[0],
                payload: bytes::Bytes::copy_from_slice(&buf[1..]),
            })
            .await
            .map_err(|err| Error::KymuxSendFailed { reason: err.0 })?;
    }

    Ok(())
}
