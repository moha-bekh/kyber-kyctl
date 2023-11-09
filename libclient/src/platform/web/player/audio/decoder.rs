use log::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::platform::player::packet::MediaPacket;

pub struct AudioDecoder {
    _output_closure: Closure<dyn FnMut(web_sys::AudioData) -> Result<(), JsValue>>,
    _error_closure: Closure<dyn FnMut(js_sys::Error) -> Result<(), JsValue>>,
    audio_decoder: web_sys::AudioDecoder,
}

pub struct AudioDecoderCallbacks {
    pub output: Box<dyn FnMut(web_sys::AudioData) -> Result<(), JsValue>>,
    pub error: Box<dyn FnMut(js_sys::Error) -> Result<(), JsValue>>,
}

impl AudioDecoder {
    pub fn new(cbs: AudioDecoderCallbacks) -> Result<Self, JsValue> {
        let output_closure = Closure::wrap(cbs.output);
        let error_closure = Closure::wrap(cbs.error);
        let output_fn = output_closure.as_ref().unchecked_ref();
        let error_fn = output_closure.as_ref().unchecked_ref();
        let audio_decoder =
            web_sys::AudioDecoder::new(&web_sys::AudioDecoderInit::new(output_fn, error_fn))?;

        Ok(Self {
            _output_closure: output_closure,
            _error_closure: error_closure,
            audio_decoder,
        })
    }

    pub fn send_packet(&mut self, packet: MediaPacket) -> Result<(), JsValue> {
        if packet.header.is_config {
            // Do not set description so that the format is AnnexB:
            // <https://www.w3.org/TR/webcodecs-avc-codec-registration/#videodecoderconfig-description>
            // About the codec string format:
            // <https://developer.mozilla.org/en-US/docs/Web/Media/Formats/codecs_parameter#basic_syntax>
            info!("Create AudioDecoder");

            self.audio_decoder
                .configure(&web_sys::AudioDecoderConfig::new(
                    super::CODEC,
                    super::CHANNELS,
                    super::SAMPLE_RATE,
                ));
        } else {
            let header = &packet.header;
            let type_ = if header.is_key {
                web_sys::EncodedAudioChunkType::Key
            } else {
                web_sys::EncodedAudioChunkType::Delta
            };
            let timestamp = header.pts as f64;

            let chunk = web_sys::EncodedAudioChunk::new(&web_sys::EncodedAudioChunkInit::new(
                &packet.data,
                timestamp,
                type_,
            ))?;
            self.audio_decoder.decode(&chunk);
        }
        Ok(())
    }

    pub async fn flush(&self) -> Result<(), JsValue> {
        let promise = self.audio_decoder.flush();
        JsFuture::from(promise).await?;
        Ok(())
    }
}
