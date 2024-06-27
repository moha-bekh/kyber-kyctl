use crate::platform::player::audio::decoder::{AudioDecoder, AudioDecoderCallbacks};
use crate::platform::player::packet::{buffer_to_packet, CodecPacket, Packet};
use crate::platform::player::FrameSink;
use crate::{Message, MessageProxy, Result};

use super::worklet::AudioWorklet;
use super::{CHANNELS, SAMPLE_RATE, SAMPLE_SIZE};

use kyaudioreg::memory::AudioRegulatorSharedMemory;
use kyaudioreg::AudioRegulatorPush;

use wasm_bindgen::prelude::*;

#[allow(unused)]
use log::{debug, error, info, warn};

pub struct AudioPlayer {
    message_proxy: MessageProxy,
    cbs: Option<AudioDecoderCallbacks>,
    audio_decoder: Option<AudioDecoder>,
}

impl AudioPlayer {
    pub(crate) const AUDIO_BUFFER_ALLOC_SIZE: u32 = 2 * SAMPLE_RATE;
    pub(crate) const AUDIO_TARGET_BUFFERING: u32 = 50 * 48;

    pub async fn create(message_proxy: MessageProxy) -> Result<(Self, AudioPlayerHandle)> {
        let alloc_size = Self::AUDIO_BUFFER_ALLOC_SIZE;
        let sample_size = SAMPLE_SIZE * CHANNELS;

        let sab = AudioRegulatorSharedMemory::create_sab(alloc_size, sample_size);

        let memory = AudioRegulatorSharedMemory::js(sab.clone(), alloc_size, sample_size);
        let mut reg = AudioRegulatorPush::new(memory, SAMPLE_RATE, Self::AUDIO_TARGET_BUFFERING);

        let message_proxy2 = message_proxy.clone();
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
                let mut tmp = vec![0u8; bytes as usize];
                frame.copy_to_with_u8_array(
                    &mut tmp,
                    web_sys::AudioDataCopyToOptions::new(0)
                        .format(web_sys::AudioSampleFormat::F32)
                        .frame_count(samples),
                );

                reg.push(&tmp);

                Ok(())
            }),
            error: Box::new(move |error| {
                info!("audio decoding error: {error:?}");

                Self::message_proxy_send(&message_proxy2, Message::AudioDecodeError);

                Ok(())
            }),
        };

        let audio_worklet = AudioWorklet::start(sab).await?;

        let player = Self {
            message_proxy,
            cbs: Some(cbs),
            audio_decoder: None,
        };
        let handle = AudioPlayerHandle {
            audio_worklet: Some(audio_worklet),
        };

        Ok((player, handle))
    }

    fn process_packet(&mut self, data: js_sys::ArrayBuffer) -> Result<()> {
        let packet = buffer_to_packet(data)?;
        match packet {
            Packet::Media(packet) => {
                assert!(self.audio_decoder.is_some());
                if let Some(audio_decoder) = &mut self.audio_decoder {
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
        }

        Ok(())
    }

    fn message_proxy_send(message_proxy: &MessageProxy, msg: Message) {
        let ret = message_proxy.send(msg.clone());
        if let Err(err) = ret {
            warn!("Failed to send {msg:?}: {err:?}");
        }
    }
}

impl FrameSink for AudioPlayer {
    fn send_frame(&mut self, data: js_sys::ArrayBuffer) -> Result<()> {
        if let Err(err) = self.process_packet(data) {
            Self::message_proxy_send(&self.message_proxy, Message::AudioDecodeError);
            Err(err)?;
        }

        Ok(())
    }
}

// Used to stop the player without the full player instance
pub(crate) struct AudioPlayerHandle {
    audio_worklet: Option<AudioWorklet>,
}

impl AudioPlayerHandle {
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(audio_worklet) = self.audio_worklet.take() {
            let ret = audio_worklet.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop audio worklet: {err:?}");
            }
        } else {
            warn!("No AudioWorklet to stop");
        }

        Ok(())
    }
}
