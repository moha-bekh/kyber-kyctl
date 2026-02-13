// Project Kyber: packet.rs
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

use kymux::types;

#[derive(Debug)]
pub enum Packet {
    Codec(CodecPacket),
    Media(MediaPacket),
    Hole(HolePacket),
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct CodecPacket {
    pub codec: u32,
    pub rotation: u8,
}

#[derive(Debug)]
pub struct MediaPacket {
    pub header: MediaPacketHeader,
    pub data: js_sys::Uint8Array, // does not include the 12-byte header
}

#[derive(Debug)]
pub struct HolePacket {
    pub missing_audio_samples: u32,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct MediaPacketHeader {
    pub is_config: bool,
    pub is_key: bool,
    pub pts: u64,
    pub size: u32,
}

#[cfg(feature = "kymux")]
impl From<types::AVPacket> for Packet {
    fn from(packet: types::AVPacket) -> Self {
        match packet {
            types::AVPacket::Codec(packet) => Packet::Codec(CodecPacket {
                codec: packet.header.codec,
                rotation: packet.header.rotation,
            }),
            types::AVPacket::Media(packet) => Packet::Media(MediaPacket {
                header: MediaPacketHeader {
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
            }),
            types::AVPacket::Hole(packet) => Packet::Hole(HolePacket {
                missing_audio_samples: packet.header.missing_audio_samples,
            }),
        }
    }
}
