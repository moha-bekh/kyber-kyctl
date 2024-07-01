use wasm_bindgen::prelude::*;

#[derive(Debug)]
pub enum Packet {
    Codec(CodecPacket),
    Media(MediaPacket),
}

#[derive(Debug)]
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
pub struct MediaPacketHeader {
    pub is_config: bool,
    pub is_key: bool,
    pub pts: u64,
    pub size: u32,
}

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
