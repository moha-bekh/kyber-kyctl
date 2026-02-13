// Project Kyber: player.rs
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

use crate::audio::decoder::{AudioDecoder, AudioDecoderCallbacks};
use crate::packet::{CodecPacket, HolePacket, Packet};
use crate::{AudioPlayerConfig, PacketSink};

use super::worklet::AudioWorklet;
use super::{CHANNELS, SAMPLE_RATE, SAMPLE_SIZE};

use kyaudioreg::memory::AudioRegulatorSharedMemory;
use kyaudioreg::AudioRegulatorPush;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use wasm_bindgen::prelude::*;

#[allow(unused)]
use log::{debug, error, info, warn};

pub struct AudioPlayer {
    listener: Rc<dyn AudioPlayerListener>,
    cbs: Option<AudioDecoderCallbacks>,
    audio_decoder: Option<AudioDecoder>,
    push_seq: u64,
    holes: Holes,
    hole: u32,
}

#[derive(Clone, Debug)]
pub enum AudioPlayerEvent {
    VolumeChanged { volume: f32 },
    MuteChanged { mute: bool },
    DecoderError,
}

pub trait AudioPlayerListener {
    fn on_event(&self, event: AudioPlayerEvent);
}

impl<F> AudioPlayerListener for F
where
    F: Fn(AudioPlayerEvent) + 'static,
{
    fn on_event(&self, event: AudioPlayerEvent) {
        self(event)
    }
}

#[derive(Debug)]
pub enum AudioPlayerCreationError {
    SharedArrayBufferNotAvailable,
    Other(JsValue),
}

impl From<JsValue> for AudioPlayerCreationError {
    fn from(value: JsValue) -> Self {
        Self::Other(value)
    }
}

impl AudioPlayer {
    pub fn available() -> bool {
        AudioRegulatorSharedMemory::available()
    }

    pub async fn create(
        listener: Box<dyn AudioPlayerListener>,
        audio_player_config: &AudioPlayerConfig,
    ) -> Result<(Self, AudioPlayerHandle), AudioPlayerCreationError> {
        let listener = Rc::<dyn AudioPlayerListener>::from(listener);
        let sample_size = SAMPLE_SIZE * CHANNELS;

        let target_buffering =
            (audio_player_config.audio_buffer as u64 * SAMPLE_RATE as u64 / 1000u64) as u32;
        let alloc_size = 2 * SAMPLE_RATE + target_buffering;

        let sab = AudioRegulatorSharedMemory::create_sab(alloc_size, sample_size)
            .map_err(|_| AudioPlayerCreationError::SharedArrayBufferNotAvailable)?;

        let memory = AudioRegulatorSharedMemory::js(sab.clone(), alloc_size, sample_size);
        let mut reg = AudioRegulatorPush::new(memory, SAMPLE_RATE, target_buffering);

        let holes = Holes::new();
        let holes2 = holes.clone();
        let mut push_seq = 0u64;

        let listener2 = listener.clone();
        let cbs = AudioDecoderCallbacks {
            output: Box::new(move |frame: web_sys::AudioData| {
                let Some(format) = frame.format() else {
                    error!("AudioData has no format");
                    return Ok(());
                };

                let expected_format = super::FORMAT;
                let expected_channels = super::CHANNELS;

                if format != expected_format {
                    error!("Unexpected format {format:?}. Wanted: '{expected_format:?}'");
                    return Ok(());
                }

                if frame.number_of_channels() != expected_channels {
                    error!(
                        "Unexpected channels {channels}. Wanted: '{expected_channels}'",
                        channels = frame.number_of_channels()
                    );
                    return Ok(());
                }

                let samples = frame.number_of_frames();
                let sample_size = frame.number_of_channels() * 4;
                let bytes = samples * sample_size;

                // TODO reuse allocation
                let copy_options = web_sys::AudioDataCopyToOptions::new(0);
                copy_options.set_format(web_sys::AudioSampleFormat::F32);
                copy_options.set_frame_count(samples);

                let mut tmp = vec![0u8; bytes as usize];
                frame.copy_to_with_u8_slice(&mut tmp[..], &copy_options)?;

                let missing_samples = holes2.pull(push_seq).unwrap_or(0);
                reg.push(missing_samples, &tmp);
                push_seq += 1;

                Ok(())
            }),
            error: Box::new(move |error| {
                info!("audio decoding error: {error:?}");

                listener2.on_event(AudioPlayerEvent::DecoderError);

                Ok(())
            }),
        };

        let audio_worklet =
            AudioWorklet::start(sab, listener.clone(), alloc_size, target_buffering).await?;

        let player = Self {
            listener,
            cbs: Some(cbs),
            audio_decoder: None,
            push_seq: 0,
            holes,
            hole: 0,
        };
        let handle = AudioPlayerHandle {
            audio_worklet: Some(audio_worklet),
        };

        Ok((player, handle))
    }

    fn process_packet(&mut self, packet: Packet) -> Result<(), JsValue> {
        match packet {
            Packet::Media(packet) => {
                assert!(self.audio_decoder.is_some());
                if let Some(audio_decoder) = &mut self.audio_decoder {
                    if !packet.header.is_config {
                        if self.hole > 0 {
                            self.holes.push(self.push_seq, self.hole);
                            self.hole = 0;
                        }
                        self.push_seq += 1;
                    }
                    audio_decoder.send_packet(packet)?;
                } else {
                    error!("Audio decoder not set");
                    return Err(JsValue::from("Audio decoder not set").into());
                }
            }
            Packet::Codec(CodecPacket { .. }) => {
                if self.audio_decoder.is_none() {
                    info!("Audio codec packet");

                    let Some(cbs) = self.cbs.take() else {
                        error!("Callbacks already consumed");
                        return Err(JsValue::from("Callbacks already consumed").into());
                    };

                    self.audio_decoder = Some(AudioDecoder::new(cbs)?);
                }
            }
            Packet::Hole(HolePacket {
                missing_audio_samples,
            }) => {
                assert_eq!(self.hole, 0); // Two consecutive hole packets is not permitted
                assert!(missing_audio_samples > 0);
                self.hole = missing_audio_samples;
            }
        }

        Ok(())
    }
}

impl PacketSink for AudioPlayer {
    fn send_packet(&mut self, packet: Packet) -> Result<(), JsValue> {
        if let Err(err) = self.process_packet(packet) {
            self.listener.on_event(AudioPlayerEvent::DecoderError);
            Err(err)?;
        }

        Ok(())
    }
}

// Used to stop the player without the full player instance
pub struct AudioPlayerHandle {
    audio_worklet: Option<AudioWorklet>,
}

impl AudioPlayerHandle {
    pub fn set_volume(&self, volume: f32) {
        let Some(audio_worklet) = &self.audio_worklet else {
            warn!("No AudioWorklet to control");
            return;
        };

        audio_worklet.set_volume(volume);
    }

    pub fn set_mute(&self, mute: bool) {
        let Some(audio_worklet) = &self.audio_worklet else {
            warn!("No AudioWorklet to control");
            return;
        };

        audio_worklet.set_mute(mute);
    }

    pub async fn stop(&mut self) {
        if let Some(audio_worklet) = self.audio_worklet.take() {
            let ret = audio_worklet.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop audio worklet: {err:?}");
            }
        } else {
            warn!("No AudioWorklet to stop");
        }
    }
}

#[derive(Debug)]
struct Hole {
    seq: u64,
    missing_audio_samples: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Holes {
    queue: Rc<RefCell<VecDeque<Hole>>>,
}

impl Holes {
    pub(crate) fn new() -> Self {
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        Self { queue }
    }

    pub(crate) fn push(&self, seq: u64, missing_audio_samples: u32) {
        assert!(missing_audio_samples > 0);
        self.queue.borrow_mut().push_back(Hole {
            seq,
            missing_audio_samples,
        });
    }

    pub(crate) fn pull(&self, expected_seq: u64) -> Option<u32> {
        let mut queue = self.queue.borrow_mut();
        if let Some(hole) = queue
            .front()
            .filter(|hole| hole.seq == expected_seq)
            .map(|hole| hole.missing_audio_samples)
        {
            queue.pop_front().unwrap();
            Some(hole)
        } else {
            None
        }
    }
}
