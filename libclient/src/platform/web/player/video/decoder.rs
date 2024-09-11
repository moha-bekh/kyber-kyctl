use log::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::platform::player::packet::MediaPacket;

pub struct VideoDecoder {
    _output_closure: Closure<dyn FnMut(web_sys::VideoFrame) -> Result<(), JsValue>>,
    _error_closure: Closure<dyn FnMut(js_sys::Error) -> Result<(), JsValue>>,
    video_decoder: web_sys::VideoDecoder,
    pending_config: Option<Vec<u8>>,
    codec: String,
    hardware_acceleration: bool,
}

pub struct VideoDecoderCallbacks {
    pub output: Box<dyn FnMut(web_sys::VideoFrame) -> Result<(), JsValue>>,
    pub error: Box<dyn FnMut(js_sys::Error) -> Result<(), JsValue>>,
}

impl VideoDecoder {
    pub fn new(
        cbs: VideoDecoderCallbacks,
        codec: String,
        hardware_acceleration: bool,
    ) -> Result<Self, JsValue> {
        let output_closure = Closure::wrap(cbs.output);
        let error_closure = Closure::wrap(cbs.error);
        let output_fn = output_closure.as_ref().unchecked_ref();
        let error_fn = error_closure.as_ref().unchecked_ref();
        let video_decoder =
            web_sys::VideoDecoder::new(&web_sys::VideoDecoderInit::new(error_fn, output_fn))?;

        Ok(Self {
            _output_closure: output_closure,
            _error_closure: error_closure,
            video_decoder,
            pending_config: None,
            codec,
            hardware_acceleration,
        })
    }

    pub fn send_packet(&mut self, packet: MediaPacket) -> Result<(), JsValue> {
        if packet.header.is_config {
            // Do not set description so that the format is AnnexB:
            // <https://www.w3.org/TR/webcodecs-avc-codec-registration/#videodecoderconfig-description>
            // About the codec string format:
            // <https://developer.mozilla.org/en-US/docs/Web/Media/Formats/codecs_parameter#basic_syntax>
            info!("Create VideoDecoder for codec {}", self.codec);

            let hardware_acceleration = if self.hardware_acceleration {
                info!("Using hardware decoding");
                web_sys::HardwareAcceleration::PreferHardware
            } else {
                info!("Using software decoding");
                web_sys::HardwareAcceleration::PreferSoftware
            };

            let decoder_config = web_sys::VideoDecoderConfig::new(&self.codec);
            decoder_config.set_optimize_for_latency(true);
            decoder_config.set_hardware_acceleration(hardware_acceleration);

            self.video_decoder.configure(&decoder_config)?;
            self.pending_config = Some(packet.data.to_vec());

            info!("Video decoder configure. Codec: {}, HardwareAcceleration: {hardware_acceleration:?}", self.codec);
        } else {
            let header = &packet.header;
            let type_ = if header.is_key {
                web_sys::EncodedVideoChunkType::Key
            } else {
                web_sys::EncodedVideoChunkType::Delta
            };
            let timestamp = header.pts as f64;
            let data = if let Some(mut data) = self.pending_config.take() {
                assert!(packet.header.is_key);
                // Concat the first keyframe with the config packet
                data.extend(packet.data.to_vec());
                js_sys::Uint8Array::from(&data[..])
            } else {
                packet.data
            };
            let chunk = web_sys::EncodedVideoChunk::new(&web_sys::EncodedVideoChunkInit::new(
                &data, timestamp, type_,
            ))?;

            let ret = self.video_decoder.decode(&chunk);
            if let Err(err) = &ret {
                error!("decode() error: {err:?}")
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn flush(&self) -> Result<(), JsValue> {
        let promise = self.video_decoder.flush();
        JsFuture::from(promise).await?;
        Ok(())
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        info!("Close video decoder");

        let ret = self.video_decoder.close();
        if let Err(err) = ret {
            warn!("Failed to close video deocder: {err:?}");
        }
    }
}
