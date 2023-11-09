use log::warn;

use crate::VideoCodec;

mod cell_buf;
mod decoder;
mod demuxer_worker;
pub(super) mod player;
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
            let promise = web_sys::VideoDecoder::is_config_supported(
                web_sys::VideoDecoderConfig::new(webcodec_str)
                    .hardware_acceleration(*acceleration)
                    .optimize_for_latency(true),
            );

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
