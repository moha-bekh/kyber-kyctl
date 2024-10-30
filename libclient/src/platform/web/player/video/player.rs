use crate::platform::dom;
use crate::platform::player::packet::{CodecPacket, Packet};
use crate::platform::player::video::decoder::{VideoDecoder, VideoDecoderCallbacks};
use crate::platform::player::video::renderer_worker::{RendererWorker, RendererWorkerHandle};
use crate::platform::player::PacketSink;
use crate::platform::VideoPlayerConfig;
use crate::{Message, MessageProxy, Result};

use super::regulator::jstime;
use super::VideoDecoderConfig;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

use wasm_bindgen::prelude::*;

#[allow(unused)]
use log::{debug, error, info, warn};

pub struct VideoPlayer {
    message_proxy: MessageProxy,
    cbs: Option<VideoDecoderCallbacks>,
    video_decoder: Option<VideoDecoder>,
    codec: String,
    hardware_acceleration: bool,
    video_rotation: Rc<AtomicU8>,
    recv_dates: Rc<RefCell<VecDeque<u64>>>,
}

impl VideoPlayer {
    pub fn create(
        message_proxy: MessageProxy,
        player_config: &VideoPlayerConfig,
        decoder_config: VideoDecoderConfig,
        notify_decoded: bool,
    ) -> Result<(Self, VideoPlayerHandle)> {
        let canvas = dom::get_canvas(&player_config.canvas)?;

        let offscreen_canvas = canvas.transfer_control_to_offscreen()?;

        let (renderer_worker, renderer_worker_handle) =
            RendererWorker::create(message_proxy.clone())?;
        renderer_worker.start(offscreen_canvas, player_config.video_buffer)?;

        let video_rotation = Rc::new(AtomicU8::new(0));
        let video_rotation_ref = video_rotation.clone();

        let codec = decoder_config.webcodec;
        let hardware_acceleration = match decoder_config.hardware_acceleration {
            web_sys::HardwareAcceleration::PreferHardware => true,
            web_sys::HardwareAcceleration::PreferSoftware => false,
            _ => {
                error!(
                    "Unexpected hardware acceleration {:?}",
                    decoder_config.hardware_acceleration
                );
                return Err(JsValue::from("Invalid hardware acceleration setting").into());
            }
        };

        let recv_dates = Rc::new(RefCell::new(VecDeque::new()));
        let recv_dates2 = recv_dates.clone();

        let message_proxy2 = message_proxy.clone();
        let message_proxy3 = message_proxy.clone();
        let cbs = VideoDecoderCallbacks {
            output: Box::new(move |frame| {
                let rotation = video_rotation_ref.load(Ordering::Relaxed);
                let pts = frame.timestamp().unwrap_or(-1.);
                let recv_date = recv_dates2.borrow_mut().pop_front().unwrap();
                renderer_worker.send_frame(frame, rotation, recv_date)?;

                if notify_decoded {
                    Self::message_proxy_send(
                        &message_proxy3,
                        Message::ImageDecoded { pts: pts as u64 },
                    );
                }

                Ok(())
            }),
            error: Box::new(move |error| {
                info!("video decoding error: {error:?}");

                Self::message_proxy_send(&message_proxy2, Message::VideoDecodeError);

                Ok(())
            }),
        };

        let player = Self {
            message_proxy,
            cbs: Some(cbs),
            video_decoder: None,
            codec,
            hardware_acceleration,
            video_rotation,
            recv_dates,
        };
        let handle = VideoPlayerHandle {
            renderer_worker_handle,
        };

        Ok((player, handle))
    }

    fn process_packet(&mut self, packet: Packet) -> Result<()> {
        match packet {
            Packet::Media(packet) => {
                assert!(self.video_decoder.is_some());
                if let Some(video_decoder) = &mut self.video_decoder {
                    if !packet.header.is_config {
                        let recv_date = jstime::now_us();
                        self.recv_dates.borrow_mut().push_back(recv_date);
                    }
                    video_decoder.send_packet(packet)?;
                } else {
                    error!("Video decoder not set");
                    return Err(JsValue::from("Video decoder not set").into());
                }
            }
            Packet::Codec(CodecPacket { rotation, .. }) => {
                self.video_rotation.store(rotation, Ordering::Relaxed);

                if self.video_decoder.is_none() {
                    info!("Video codec packet");

                    let Some(cbs) = self.cbs.take() else {
                        error!("Callbacks already consumed");
                        return Err(JsValue::from("Callbacks already consumed").into());
                    };

                    self.video_decoder = Some(VideoDecoder::new(
                        cbs,
                        self.codec.clone(),
                        self.hardware_acceleration,
                    )?);
                }
            }
            Packet::Hole(_) => panic!("Unexpected hole packet in video stream"),
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

impl PacketSink for VideoPlayer {
    fn send_packet(&mut self, packet: Packet) -> Result<()> {
        if let Err(err) = self.process_packet(packet) {
            Self::message_proxy_send(&self.message_proxy, Message::VideoDecodeError);
            Err(err)?;
        }

        Ok(())
    }
}

// Used to stop the player without the full player instance
pub(crate) struct VideoPlayerHandle {
    renderer_worker_handle: RendererWorkerHandle,
}

impl VideoPlayerHandle {
    pub async fn stop(&mut self) -> Result<()> {
        let ret = self.renderer_worker_handle.stop();
        if let Err(err) = ret {
            warn!("Failed to stop video renderer: {err:?}");
        }

        Ok(())
    }
}
