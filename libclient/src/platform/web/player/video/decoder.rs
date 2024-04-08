use log::{error, info};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::HardwareAcceleration;

use crate::platform::player::packet::MediaPacket;
use crate::platform::reflect;

#[wasm_bindgen(raw_module = "./video_decoder.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn decoder_configure(
        decoder: &web_sys::VideoDecoder,
        config: &web_sys::VideoDecoderConfig,
    ) -> Result<(), JsValue>;

    #[wasm_bindgen(catch)]
    fn decoder_decode(
        decoder: &web_sys::VideoDecoder,
        chunk: &web_sys::EncodedVideoChunk,
    ) -> Result<(), JsValue>;
}

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

            decoder_configure(
                &self.video_decoder,
                web_sys::VideoDecoderConfig::new(&self.codec)
                    .optimize_for_latency(true)
                    .hardware_acceleration(hardware_acceleration),
            )?;
            self.pending_config = Some(packet.data.to_vec());

            Self::notify_configured(&self.codec, hardware_acceleration)?;
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

            let ret = decoder_decode(&self.video_decoder, &chunk);
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

    fn notify_configured(
        codec: &str,
        hardware_acceleration: HardwareAcceleration,
    ) -> Result<(), JsValue> {
        let js_self = js_sys::eval("self")?.dyn_into::<web_sys::DedicatedWorkerGlobalScope>()?;

        let data = js_sys::Object::new();
        reflect::set(&data, "codec", codec.into())?;
        reflect::set(&data, "hardware_acceleration", hardware_acceleration.into())?;

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", "video_decoder_configured".into())?;
        reflect::set(&msg, "data", data.into())?;
        js_self.post_message(&msg)?;

        Ok(())
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        info!("Close video decoder");
        self.video_decoder.close();
    }
}
