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

use crate::platform::player::packet::{CodecPacket, MediaPacket, MediaPacketHeader, Packet};

use wasm_bindgen::prelude::*;

pub(crate) fn buffer_to_packet(frame: js_sys::ArrayBuffer) -> Result<Packet, JsValue> {
    let buf = js_sys::Uint8Array::new(&frame).to_vec();

    let is_media_packet = buf[0] & 0x80 != 0;
    let pkt = if is_media_packet {
        let size = u32::from_be_bytes(buf[8..12].try_into().unwrap());

        let pts_and_flags = u64::from_be_bytes(buf[..8].try_into().unwrap());
        let is_config = (pts_and_flags & 0x40_00_00_00_00_00_00_00) != 0;
        let is_key = (pts_and_flags & 0x20_00_00_00_00_00_00_00) != 0;
        let pts = pts_and_flags & 0x1F_FF_FF_FF_FF_FF_FF_FF;

        let header = MediaPacketHeader {
            is_config,
            is_key,
            pts,
            size,
        };

        let slice = &buf[12..];
        let data = js_sys::Uint8Array::from(slice);

        Packet::Media(MediaPacket { header, data })
    } else {
        let codec = u32::from_be_bytes(buf[..4].try_into().unwrap());
        let rotation = buf[4];
        Packet::Codec(CodecPacket { codec, rotation })
    };

    Ok(pkt)
}
