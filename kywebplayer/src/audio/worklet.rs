// Project Kyber: worklet.rs
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

use std::cell::RefCell;
use std::rc::Rc;

use log::warn;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use super::player::{AudioPlayerEvent, AudioPlayerListener};
use crate::reflect;
use crate::AudioResumeState;

struct GainNodeConfig {
    gain: f32,
    mute: bool,
}

pub struct AudioWorklet {
    ctx: web_sys::AudioContext,
    gain_node: web_sys::GainNode,
    gain_cfg: RefCell<GainNodeConfig>,
    node: web_sys::AudioWorkletNode,
    listener: Rc<dyn AudioPlayerListener>,
}

impl AudioWorklet {
    pub async fn start(
        sab: js_sys::SharedArrayBuffer,
        listener: Rc<dyn AudioPlayerListener>,
        alloc_size: u32,
        target_buffering: u32,
    ) -> Result<Self, JsValue> {
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
        listener.on_event(AudioPlayerEvent::VolumeChanged { volume: gain });

        let mute = gain <= 0.0;
        listener.on_event(AudioPlayerEvent::MuteChanged { mute });

        // Link everything
        node.connect_with_audio_node(&gain_node)?;
        gain_node.connect_with_audio_node(&ctx.destination())?;

        // Fire a non-blocking resume attempt. If sticky activation is already
        // set (normal connect flow) the browser resolves the promise silently.
        // If not (OIDC redirect), the browser refuses and the caller probes
        // via AudioPlayerHandle::try_resume() until the user provides a gesture.
        if let Err(e) = ctx.resume() {
            warn!("AudioContext.resume() failed synchronously: {:?}", e);
        }

        Ok(Self {
            ctx,
            gain_node,
            gain_cfg: RefCell::new(GainNodeConfig { gain, mute }),
            node,
            listener,
        })
    }

    pub(super) fn ctx(&self) -> web_sys::AudioContext {
        self.ctx.clone()
    }

    /// See [`crate::AudioPlayerHandle::try_resume`].
    pub(super) fn try_resume(&self) -> AudioResumeState {
        if self.ctx.state() == web_sys::AudioContextState::Running {
            return AudioResumeState::Running;
        }
        // resume() returns a Promise; the state only flips to Running once it
        // resolves, so we still report Suspended here and let the caller poll.
        if let Err(e) = self.ctx.resume() {
            warn!("AudioContext.resume() failed synchronously: {:?}", e);
        }
        AudioResumeState::Suspended
    }

    pub fn set_volume(&self, volume: f32) {
        let mut gain_cfg = self.gain_cfg.borrow_mut();

        if !gain_cfg.mute {
            self.gain_node.gain().set_value(volume);
        }

        gain_cfg.gain = volume;

        self.listener
            .on_event(AudioPlayerEvent::VolumeChanged { volume });
    }

    pub fn set_mute(&self, mute: bool) {
        let mut gain_cfg = self.gain_cfg.borrow_mut();

        if mute {
            self.gain_node.gain().set_value(0.0);
        } else {
            self.gain_node.gain().set_value(gain_cfg.gain);
        }

        gain_cfg.mute = mute;

        self.listener
            .on_event(AudioPlayerEvent::MuteChanged { mute });
    }

    pub async fn stop(self) -> Result<(), JsValue> {
        // Suspend before close; ignore the error so close() always runs.
        if let Ok(promise) = self.ctx.suspend() {
            if let Err(e) = JsFuture::from(promise).await {
                warn!("AudioContext.suspend() failed: {:?}", e);
            }
        }

        let promise = self.ctx.close()?;
        JsFuture::from(promise).await?;

        self.node.disconnect()?;

        Ok(())
    }
}
