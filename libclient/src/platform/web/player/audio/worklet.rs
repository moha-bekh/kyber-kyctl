use std::cell::RefCell;

use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use crate::platform::reflect;
use crate::{Error, Message, MessageSender};

struct GainNodeConfig {
    gain: f32,
    mute: bool,
}

pub(crate) struct AudioWorklet {
    ctx: web_sys::AudioContext,
    gain_node: web_sys::GainNode,
    gain_cfg: RefCell<GainNodeConfig>,
    node: web_sys::AudioWorkletNode,
    message_sender: MessageSender,
}

impl AudioWorklet {
    pub async fn start(
        sab: js_sys::SharedArrayBuffer,
        message_sender: MessageSender,
        alloc_size: u32,
        target_buffering: u32,
    ) -> Result<Self, Error> {
        // AudioContext
        let options = web_sys::AudioContextOptions::new();
        options.set_sample_rate(super::SAMPLE_RATE as f32);
        options.set_latency_hint(&JsValue::from("interactive"));

        let ctx = web_sys::AudioContext::new_with_context_options(&options)?;

        // AudioProcessor/AudioRenderer
        let promise = ctx.audio_worklet()?.add_module("audio_worklet.js")?;
        JsFuture::from(promise).await?;

        let processor_options = js_sys::Object::new();
        reflect::set(&processor_options, "sab", sab.into())?;

        reflect::set(&processor_options, "sampleRate", super::SAMPLE_RATE.into())?;

        reflect::set(&processor_options, "sampleSize", super::SAMPLE_SIZE.into())?;

        reflect::set(&processor_options, "allocSize", alloc_size.into())?;

        reflect::set(
            &processor_options,
            "targetBuffering",
            target_buffering.into(),
        )?;

        let options = web_sys::AudioWorkletNodeOptions::new();
        options.set_output_channel_count(&JsValue::from(js_sys::Array::of1(&JsValue::from(
            super::CHANNELS,
        ))));
        options.set_processor_options(Some(&processor_options));

        let node = web_sys::AudioWorkletNode::new_with_options(&ctx, "AudioRenderer", &options)?;

        // AudioGain
        let gain_node: web_sys::GainNode = ctx.create_gain()?;

        let gain = gain_node.gain().value();
        message_sender.send(Message::AudioVolumeChanged { volume: gain })?;

        let mute = gain <= 0.0;
        message_sender.send(Message::AudioMuteChanged { mute })?;

        // Link everything
        node.connect_with_audio_node(&gain_node)?;
        gain_node.connect_with_audio_node(&ctx.destination())?;

        // Resume AudioContext
        let promise = ctx.resume()?;
        JsFuture::from(promise).await?;

        Ok(Self {
            ctx,
            gain_node,
            gain_cfg: RefCell::new(GainNodeConfig { gain, mute }),
            node,
            message_sender,
        })
    }

    pub fn set_volume(&self, volume: f32) -> Result<(), Error> {
        let mut gain_cfg = self.gain_cfg.borrow_mut();

        if !gain_cfg.mute {
            self.gain_node.gain().set_value(volume);
        }

        gain_cfg.gain = volume;

        self.message_sender
            .send(Message::AudioVolumeChanged { volume })?;

        Ok(())
    }

    pub fn set_mute(&self, mute: bool) -> Result<(), Error> {
        let mut gain_cfg = self.gain_cfg.borrow_mut();

        if mute {
            self.gain_node.gain().set_value(0.0);
        } else {
            self.gain_node.gain().set_value(gain_cfg.gain);
        }

        gain_cfg.mute = mute;

        self.message_sender
            .send(Message::AudioMuteChanged { mute })?;

        Ok(())
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
