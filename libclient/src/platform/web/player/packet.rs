#[derive(Debug)]
pub enum Packet {
    Codec(CodecPacket),
    Media(MediaPacket),
    Hole(HolePacket),
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
pub struct HolePacket {
    pub missing_audio_samples: u32,
}

#[derive(Debug)]
pub struct MediaPacketHeader {
    pub is_config: bool,
    pub is_key: bool,
    pub pts: u64,
    pub size: u32,
}

#[cfg(feature = "kymux")]
impl From<kymux::AVPacket> for Packet {
    fn from(packet: kymux::AVPacket) -> Self {
        match packet {
            kymux::AVPacket::Codec(packet) => Packet::Codec(CodecPacket {
                codec: packet.header.codec,
                rotation: packet.header.rotation,
            }),
            kymux::AVPacket::Media(packet) => Packet::Media(MediaPacket {
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
            kymux::AVPacket::Hole(packet) => Packet::Hole(HolePacket {
                missing_audio_samples: packet.header.missing_audio_samples,
            }),
        }
    }
}
