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

use crate::packet::{CodecPacket, Packet};
use crate::video::decoder::{VideoDecoder, VideoDecoderCallbacks};
use crate::video::renderer_worker::{RendererWorker, RendererWorkerHandle};
use crate::{PacketSink, VideoPlayerConfig};

use super::VideoDecoderConfig;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};

use kywasmtime::Instant;
use wasm_bindgen::prelude::*;

#[allow(unused)]
use log::{debug, error, info, warn};

pub struct VideoPlayer {
    listener: Rc<dyn VideoPlayerListener>,
    cbs: Option<VideoDecoderCallbacks>,
    video_decoder: Option<VideoDecoder>,
    codec: String,
    hardware_acceleration: bool,
    video_rotation: Rc<AtomicU8>,
    recv_dates: Rc<RefCell<VecDeque<i64>>>,
    metrics: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoPlayerMetric {
    FrameReceived,
    FrameDecoding,
    FrameDecoded,
    FrameSkipped,
    FrameDisplayed,
}

#[derive(Clone, Debug)]
pub enum VideoPlayerEvent {
    CanvasResized {
        width: i32,
        height: i32,
        frame_coded_width: i32,
        frame_coded_height: i32,
    },
    Metric {
        pts: u64,
        ts: i64,
        data: VideoPlayerMetric,
    },
    FrameDecoded {
        pts: u64,
    },
    DecoderError,
}

pub trait VideoPlayerListener {
    fn on_event(&self, event: VideoPlayerEvent);
}

impl<F> VideoPlayerListener for F
where
    F: Fn(VideoPlayerEvent) + 'static,
{
    fn on_event(&self, event: VideoPlayerEvent) {
        self(event)
    }
}

impl VideoPlayer {
    pub fn create(
        listener: Box<dyn VideoPlayerListener>,
        player_config: &VideoPlayerConfig,
        decoder_config: VideoDecoderConfig,
        metrics: bool,
    ) -> Result<(Self, VideoPlayerHandle), JsValue> {
        let listener = Rc::<dyn VideoPlayerListener>::from(listener);

        let document = web_sys::window()
            .and_then(|w| w.document())
            .ok_or_else(|| JsValue::from(js_sys::Error::new("No window or document")))?;
        let canvas = document
            .get_element_by_id(&player_config.canvas)
            .ok_or_else(|| JsValue::from(js_sys::Error::new("Canvas not found")))?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;

        let offscreen_canvas = canvas.transfer_control_to_offscreen()?;

        let (renderer_worker, renderer_worker_handle) =
            RendererWorker::create(&player_config.app_crate_name, listener.clone())?;
        renderer_worker.start(offscreen_canvas, player_config.video_buffer, metrics)?;

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

        let notify_decoded = player_config.notify_decoded;
        let listener2 = listener.clone();
        let listener3 = listener.clone();
        let cbs = VideoDecoderCallbacks {
            output: Box::new(move |frame| {
                let now = Instant::now().raw_us() as i64;
                let rotation = video_rotation_ref.load(Ordering::Relaxed);
                let pts = frame.timestamp().unwrap_or(-1.);
                let recv_date = recv_dates2.borrow_mut().pop_front().unwrap();

                if metrics {
                    if let Some(pts) = frame.timestamp() {
                        let pts = pts as u64;
                        listener2.on_event(VideoPlayerEvent::Metric {
                            pts,
                            ts: now,
                            data: VideoPlayerMetric::FrameDecoded,
                        });
                    }
                }

                renderer_worker.send_frame(frame, rotation, recv_date)?;

                if notify_decoded {
                    listener2.on_event(VideoPlayerEvent::FrameDecoded { pts: pts as u64 });
                }

                Ok(())
            }),
            error: Box::new(move |error| {
                info!("video decoding error: {error:?}");

                listener3.on_event(VideoPlayerEvent::DecoderError);

                Ok(())
            }),
        };

        let player = Self {
            listener,
            cbs: Some(cbs),
            video_decoder: None,
            codec,
            hardware_acceleration,
            video_rotation,
            recv_dates,
            metrics,
        };
        let handle = VideoPlayerHandle {
            renderer_worker_handle,
        };

        Ok((player, handle))
    }

    fn process_packet(&mut self, packet: Packet) -> Result<(), JsValue> {
        match packet {
            Packet::Media(packet) => {
                assert!(self.video_decoder.is_some());
                if let Some(video_decoder) = &mut self.video_decoder {
                    if !packet.header.is_config {
                        let recv_date = Instant::now().raw_us() as i64;
                        self.recv_dates.borrow_mut().push_back(recv_date);

                        if self.metrics {
                            let metric = VideoPlayerEvent::Metric {
                                pts: packet.header.pts,
                                ts: recv_date,
                                data: VideoPlayerMetric::FrameReceived,
                            };
                            self.listener.on_event(metric);
                        }
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
                        self.listener.clone(),
                        cbs,
                        self.codec.clone(),
                        self.hardware_acceleration,
                        self.metrics,
                    )?);
                }
            }
            Packet::Hole(_) => panic!("Unexpected hole packet in video stream"),
        }

        Ok(())
    }
}

impl PacketSink for VideoPlayer {
    fn send_packet(&mut self, packet: Packet) -> Result<(), JsValue> {
        if let Err(err) = self.process_packet(packet) {
            self.listener.on_event(VideoPlayerEvent::DecoderError);
            Err(err)?;
        }

        Ok(())
    }
}

// Used to stop the player without the full player instance
pub struct VideoPlayerHandle {
    renderer_worker_handle: RendererWorkerHandle,
}

impl VideoPlayerHandle {
    pub async fn stop(&mut self) {
        let ret = self.renderer_worker_handle.stop();
        if let Err(err) = ret {
            warn!("Failed to stop video renderer: {err:?}");
        }
    }
}
