use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use crate::platform::reflect;

pub(crate) struct AudioWorklet {
    ctx: web_sys::AudioContext,
    node: web_sys::AudioWorkletNode,
}

impl AudioWorklet {
    pub async fn start(
        sab: js_sys::SharedArrayBuffer,
        sample_rate: u32,
        channel_count: u32,
    ) -> Result<Self, JsValue> {
        let ctx = web_sys::AudioContext::new_with_context_options(
            web_sys::AudioContextOptions::new()
                .sample_rate(sample_rate as f32)
                .latency_hint(&JsValue::from("interactive")),
        )?;

        let promise = ctx.audio_worklet()?.add_module("audio_worklet.js")?;
        JsFuture::from(promise).await?;

        let processor_options = js_sys::Object::new();
        reflect::set(&processor_options, "sab", sab.into())?;

        let node = web_sys::AudioWorkletNode::new_with_options(
            &ctx,
            "AudioRenderer",
            web_sys::AudioWorkletNodeOptions::new()
                .output_channel_count(&JsValue::from(js_sys::Array::of1(&JsValue::from(
                    channel_count,
                ))))
                .processor_options(Some(&processor_options)),
        )?;
        node.connect_with_audio_node(&ctx.destination())?;

        let promise = ctx.resume()?;
        JsFuture::from(promise).await?;

        Ok(Self { ctx, node })
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
