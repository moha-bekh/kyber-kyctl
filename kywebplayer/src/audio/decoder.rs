// Project Kyber: decoder.rs
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

use log::{error, info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::packet::MediaPacket;

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
        let error_fn = error_closure.as_ref().unchecked_ref();
        let audio_decoder =
            web_sys::AudioDecoder::new(&web_sys::AudioDecoderInit::new(error_fn, output_fn))?;

        Ok(Self {
            _output_closure: output_closure,
            _error_closure: error_closure,
            audio_decoder,
        })
    }

    pub fn send_packet(&mut self, packet: MediaPacket) -> Result<(), JsValue> {
        if packet.header.is_config {
            info!("Create AudioDecoder");

            self.audio_decoder
                .configure(&web_sys::AudioDecoderConfig::new(
                    super::CODEC,
                    super::CHANNELS,
                    super::SAMPLE_RATE,
                ))?;
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

            let ret = self.audio_decoder.decode(&chunk);
            if let Err(err) = &ret {
                error!("Audio decode() error: {err:?}")
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn flush(&self) -> Result<(), JsValue> {
        let promise = self.audio_decoder.flush();
        JsFuture::from(promise).await?;
        Ok(())
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        info!("Close audio decoder");

        let ret = self.audio_decoder.close();
        if let Err(err) = ret {
            warn!("Failed to close audio deocder: {err:?}");
        }
    }
}
