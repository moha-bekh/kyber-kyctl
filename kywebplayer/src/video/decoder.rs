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
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use super::player::{VideoPlayerEvent, VideoPlayerListener, VideoPlayerMetric};
use super::{VideoCodec, VideoDecoderConfig};
use crate::packet::{self, MediaPacket};
use kywasmtime::Instant;

pub struct VideoDecoder {
    listener: Rc<dyn VideoPlayerListener>,
    _output_closure: Closure<dyn FnMut(web_sys::VideoFrame) -> Result<(), JsValue>>,
    _error_closure: Closure<dyn FnMut(js_sys::Error) -> Result<(), JsValue>>,
    video_decoder: web_sys::VideoDecoder,
    pending_config: Option<Vec<u8>>,
    kymux_codec: u32,
    force_software_decoding: bool,
    yuv444: bool,
    metrics: bool,
    supported_video_codecs: Vec<VideoDecoderConfig>,
}

pub struct VideoDecoderCallbacks {
    pub output: Box<dyn FnMut(web_sys::VideoFrame) -> Result<(), JsValue>>,
    pub error: Box<dyn FnMut(js_sys::Error) -> Result<(), JsValue>>,
}

impl VideoDecoder {
    pub fn new(
        listener: Rc<dyn VideoPlayerListener>,
        cbs: VideoDecoderCallbacks,
        kymux_codec: u32,
        force_software_decoding: bool,
        yuv444: bool,
        metrics: bool,
        supported_video_codecs: Vec<VideoDecoderConfig>,
    ) -> Result<Self, JsValue> {
        let output_closure = Closure::wrap(cbs.output);
        let error_closure = Closure::wrap(cbs.error);
        let output_fn = output_closure.as_ref().unchecked_ref();
        let error_fn = error_closure.as_ref().unchecked_ref();
        let video_decoder =
            web_sys::VideoDecoder::new(&web_sys::VideoDecoderInit::new(error_fn, output_fn))?;

        Ok(Self {
            listener,
            _output_closure: output_closure,
            _error_closure: error_closure,
            video_decoder,
            pending_config: None,
            kymux_codec,
            force_software_decoding,
            yuv444,
            metrics,
            supported_video_codecs,
        })
    }

    fn get_video_codec(kymux_codec: u32) -> Option<VideoCodec> {
        match kymux_codec {
            packet::CODEC_H264 => Some(VideoCodec::H264),
            packet::CODEC_H265 => Some(VideoCodec::HEVC),
            packet::CODEC_AV1 => Some(VideoCodec::AV1),
            _ => None,
        }
    }

    fn create_video_decoder_config(&self) -> Option<web_sys::VideoDecoderConfig> {
        let video_codec = Self::get_video_codec(self.kymux_codec)?;

        info!("Create VideoDecoder for codec {video_codec:?}");
        let desc = super::get_video_decoder_config(
            &self.supported_video_codecs,
            video_codec,
            self.yuv444,
            self.force_software_decoding,
        )?;

        info!(
            "Video decoder configure. Codec: {}, HardwareAcceleration: {:?}",
            &desc.webcodec, desc.hardware_acceleration,
        );

        // About the codec string format:
        // <https://developer.mozilla.org/en-US/docs/Web/Media/Formats/codecs_parameter#basic_syntax>
        // Do not set description so that the format is AnnexB:
        // <https://www.w3.org/TR/webcodecs-avc-codec-registration/#videodecoderconfig-description>
        let decoder_config = web_sys::VideoDecoderConfig::new(&desc.webcodec);
        decoder_config.set_hardware_acceleration(desc.hardware_acceleration);
        decoder_config.set_optimize_for_latency(true);
        Some(decoder_config)
    }

    pub fn send_packet(&mut self, packet: MediaPacket) -> Result<(), JsValue> {
        if packet.header.is_config {
            let Some(decoder_config) = self.create_video_decoder_config() else {
                return Err(JsError::new("No decoder found").into());
            };

            self.video_decoder.configure(&decoder_config)?;
            self.pending_config = Some(packet.data.to_vec());
        } else {
            let header = &packet.header;
            let type_ = if header.is_key {
                web_sys::EncodedVideoChunkType::Key
            } else {
                web_sys::EncodedVideoChunkType::Delta
            };
            let timestamp = header.pts as f64;
            let data = if let Some(mut data) = self.pending_config.take() {
                info!("Video keyframe received");
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

            if self.metrics {
                let metric = VideoPlayerEvent::Metric {
                    pts: header.pts,
                    ts: Instant::now().raw_us() as i64,
                    data: VideoPlayerMetric::FrameDecoding,
                };
                self.listener.on_event(metric);
            }

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
