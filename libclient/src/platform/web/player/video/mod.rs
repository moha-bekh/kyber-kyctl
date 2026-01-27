// Project Kyber: mod.rs
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

use log::{error, warn};

use crate::{Error, Result, VideoCodec, VideoSessionConfig};

mod cell_buf;
mod decoder;
pub(super) mod player;
mod regulator;
mod renderer;
mod renderer_worker;

#[derive(Clone, Debug)]
pub struct VideoDecoderConfig {
    pub codec: VideoCodec,
    pub yuv444: bool,
    pub webcodec: String,
    pub hardware_acceleration: web_sys::HardwareAcceleration,
}

pub(crate) async fn supported_video_codecs(force_software: bool) -> Vec<VideoDecoderConfig> {
    let codecs = &[
        (VideoCodec::H264, false, "avc1.64002a"),
        (VideoCodec::H264, true, "avc1.f4002a"),
        (VideoCodec::HEVC, false, "hev1.1.2.H123.90"),
        (VideoCodec::HEVC, true, "hev1.4.10.L123.9e.8"),
    ];

    let accelerations = if force_software {
        vec![web_sys::HardwareAcceleration::PreferSoftware]
    } else {
        vec![
            web_sys::HardwareAcceleration::PreferHardware,
            web_sys::HardwareAcceleration::PreferSoftware,
        ]
    };

    let mut configs = vec![];

    for (codec, yuv444, webcodec_str) in codecs {
        for acceleration in &accelerations {
            // Run promise
            let decoder_config = web_sys::VideoDecoderConfig::new(webcodec_str);
            decoder_config.set_hardware_acceleration(*acceleration);
            decoder_config.set_optimize_for_latency(true);

            let promise = web_sys::VideoDecoder::is_config_supported(&decoder_config);

            let promise_ret = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(ret) => ret,
                Err(err) => {
                    warn!("Failed to query if {webcodec_str} is supported: {err:?}");
                    continue;
                }
            };

            // Unpack promise result
            let js_value = match js_sys::Reflect::get(&promise_ret, &"supported".into()) {
                Ok(ret) => ret,
                Err(err) => {
                    warn!("Failed to get 'supported' attribute: {err:?}");
                    continue;
                }
            };

            let Some(supported) = js_value.as_bool() else {
                warn!("'supported' attribute isn't a boolean");
                continue;
            };

            if supported {
                configs.push(VideoDecoderConfig {
                    codec: *codec,
                    yuv444: *yuv444,
                    webcodec: webcodec_str.to_string(),
                    hardware_acceleration: *acceleration,
                })
            }
        }
    }

    configs
}

pub(crate) async fn get_video_decoder_config(
    video_config: &VideoSessionConfig,
) -> Result<VideoDecoderConfig> {
    let supported_video_codecs = supported_video_codecs(video_config.player.software_decode).await;

    let supported_video_codecs: Vec<_> = supported_video_codecs
        .into_iter()
        .filter(|config| config.codec == video_config.codec && config.yuv444 == video_config.yuv444)
        .collect();

    if supported_video_codecs.len() > 1 {
        let video_decoder_config = supported_video_codecs.into_iter().find(|config| {
            config.hardware_acceleration == web_sys::HardwareAcceleration::PreferHardware
        });

        match video_decoder_config {
            Some(video_decoder_config) => Ok(video_decoder_config),
            None => {
                error!(
                    "Expected to find a configuration with hardware configuration, but got none"
                );
                Err(Error::NoValidVideoDecoderConfig)
            }
        }
    } else {
        Ok(supported_video_codecs[0].clone())
    }
}
