use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use crate::platform::player::AudioPlayer;
use crate::platform::reflect;
use crate::MessageProxy;

pub(crate) struct AudioWorklet {
    ctx: web_sys::AudioContext,
    node: web_sys::AudioWorkletNode,
    message_proxy: MessageProxy,
}

impl AudioWorklet {
    pub async fn start(
        sab: js_sys::SharedArrayBuffer,
        message_proxy: MessageProxy,
    ) -> Result<Self, JsValue> {
        let ctx = web_sys::AudioContext::new_with_context_options(
            web_sys::AudioContextOptions::new()
                .sample_rate(super::SAMPLE_RATE as f32)
                .latency_hint(&JsValue::from("interactive")),
        )?;

        let promise = ctx.audio_worklet()?.add_module("audio_worklet.js")?;
        JsFuture::from(promise).await?;

        let processor_options = js_sys::Object::new();
        reflect::set(&processor_options, "sab", sab.into())?;

        reflect::set(&processor_options, "sampleRate", super::SAMPLE_RATE.into())?;

        reflect::set(&processor_options, "sampleSize", super::SAMPLE_SIZE.into())?;

        reflect::set(
            &processor_options,
            "allocSize",
            AudioPlayer::AUDIO_BUFFER_ALLOC_SIZE.into(),
        )?;

        reflect::set(
            &processor_options,
            "targetBuffering",
            AudioPlayer::AUDIO_TARGET_BUFFERING.into(),
        )?;

        let node = web_sys::AudioWorkletNode::new_with_options(
            &ctx,
            "AudioRenderer",
            web_sys::AudioWorkletNodeOptions::new()
                .output_channel_count(&JsValue::from(js_sys::Array::of1(&JsValue::from(
                    super::CHANNELS,
                ))))
                .processor_options(Some(&processor_options)),
        )?;
        node.connect_with_audio_node(&ctx.destination())?;

        let promise = ctx.resume()?;
        JsFuture::from(promise).await?;

        Ok(Self { ctx, node, message_proxy })
    }

    pub async fn stop(self) -> Result<(), JsValue> {
        let promise = self.ctx.suspend()?;
        JsFuture::from(promise).await?;

        let promise = self.ctx.close()?;
        JsFuture::from(promise).await?;

        self.node.disconnect()?;

        Ok(())
    }
}
