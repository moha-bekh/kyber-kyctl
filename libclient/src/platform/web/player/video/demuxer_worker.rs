use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use log::{error, info, warn};

use tokio::sync::mpsc::{self, error::TrySendError};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::platform::player::packet::buffer_to_packet;
use crate::platform::player::packet::{CodecPacket, Packet};
use crate::platform::reflect::{self, get_bool, get_f64, get_string};
use crate::{Message, MessageProxy};

use super::decoder::{VideoDecoder, VideoDecoderCallbacks};
use super::renderer_worker::RendererWorker;
use super::VideoDecoderConfig;

#[wasm_bindgen]
pub struct VideoDemuxerWorkerCtx {
    tx: mpsc::Sender<js_sys::ArrayBuffer>,
    rx: Option<mpsc::Receiver<js_sys::ArrayBuffer>>,
}

#[wasm_bindgen]
impl VideoDemuxerWorkerCtx {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(32);

        Self { tx, rx: Some(rx) }
    }

    pub async fn on_message(&mut self, event: web_sys::MessageEvent) -> Result<(), JsValue> {
        let msg = &event.data().dyn_into::<js_sys::Object>()?;
        let msg_type = get_string(msg, "type")?;

        match msg_type.as_str() {
            DemuxerWorker::TYPE_START => {
                let canvas = js_sys::Reflect::get(msg, &"canvas".into())?
                    .dyn_into::<web_sys::OffscreenCanvas>()?;

                let codec = get_string(msg, "codec")?;
                let hardware_acceleration = get_bool(msg, "hardware_acceleration")?;

                let Some(rx) = self.rx.take() else {
                    warn!("VideoDemuxerWorker has already received Start message");
                    return Err(JsError::new("Start message already received").into());
                };

                spawn_local(async move {
                    let ret =
                        Self::demux_process_loop(rx, canvas, codec, hardware_acceleration).await;
                    if let Err(err) = ret {
                        error!("Video demux loop ended with error {err:?}");

                        let ret = Self::forward_error_to_parent("decode_error", err);
                        if let Err(err) = &ret {
                            warn!("Failed to forward error to parent: {err:?}");
                        }
                    }
                });
            }
            DemuxerWorker::TYPE_FRAME => {
                let frame = js_sys::Reflect::get(msg, &"frame".into())?
                    .dyn_into::<js_sys::ArrayBuffer>()?;

                match self.tx.try_send(frame) {
                    Ok(_) => {}
                    Err(TrySendError::Closed(_)) => {
                        warn!("Failed to forward VideoFrame to demuxer task: channel closed");
                    }
                    Err(TrySendError::Full(_)) => {
                        warn!("VideoFrame queue full: frame dropped");
                    }
                }
            }
            _ => {
                return Err(JsValue::from(format!(
                    "Unexpected message type: {msg_type}"
                )));
            }
        }

        Ok(())
    }

    async fn demux_process_loop(
        mut rx: mpsc::Receiver<js_sys::ArrayBuffer>,
        canvas: web_sys::OffscreenCanvas,
        codec: String,
        hardware_acceleration: bool,
    ) -> Result<(), JsValue> {
        let mut video_decoder: Option<VideoDecoder> = None;
        let video_rotation = Rc::new(AtomicU8::new(0));

        let renderer_worker = RendererWorker::new()?;
        renderer_worker.start(canvas)?;

        let video_rotation_decoder = video_rotation.clone();

        let mut cbs = Some(VideoDecoderCallbacks {
            output: Box::new(move |frame| {
                let rotation = video_rotation_decoder.load(Ordering::Relaxed);
                renderer_worker.send_frame(frame, rotation)?;
                Ok(())
            }),
            error: Box::new(move |error| {
                let ret = Self::forward_error_to_parent("decode_error", error.into());
                if let Err(err) = &ret {
                    warn!("Failed to forward error to parent: {err:?}");
                }

                Ok(())
            }),
        });

        while let Some(buffer) = rx.recv().await {
            match buffer_to_packet(buffer)? {
                Packet::Media(packet) => {
                    assert!(video_decoder.is_some());
                    if let Some(video_decoder) = &mut video_decoder {
                        video_decoder.send_packet(packet)?;
                    } else {
                        error!("Video decoder not set");
                        return Err(JsError::new("Video decoder not set").into());
                    }
                }
                Packet::Codec(CodecPacket { rotation }) => {
                    video_rotation.store(rotation, Ordering::Relaxed);

                    if video_decoder.is_none() {
                        info!("Video codec packet");

                        let Some(cbs) = cbs.take() else {
                            error!("Callbacks already consumed");
                            return Err(JsError::new("Callbacks already consumed").into());
                        };

                        video_decoder = Some(VideoDecoder::new(
                            cbs,
                            codec.clone(),
                            hardware_acceleration,
                        )?);
                    }
                }
            }
        }

        Ok(())
    }

    fn forward_error_to_parent(name: &str, error: JsValue) -> Result<(), JsValue> {
        let js_self = js_sys::eval("self")?.dyn_into::<web_sys::DedicatedWorkerGlobalScope>()?;

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", name.into())?;
        reflect::set(&msg, "error", error)?;
        js_self.post_message(&msg)?;

        Ok(())
    }
}

pub struct DemuxerWorker {
    worker: web_sys::Worker,
    running: Rc<AtomicBool>,
}

impl DemuxerWorker {
    const TYPE_START: &str = "start";
    const TYPE_FRAME: &str = "frame";

    pub fn new(message_proxy: MessageProxy) -> Result<DemuxerWorker, JsValue> {
        let worker = web_sys::Worker::new_with_options(
            "./video_demuxer_worker.js",
            web_sys::WorkerOptions::new().type_(web_sys::WorkerType::Module),
        )?;

        let running = Rc::new(AtomicBool::new(true));

        // Get messages from worker
        let running_msg_handler = running.clone();

        let worker_message_cb =
            Closure::<dyn FnMut(_)>::new(move |event: web_sys::MessageEvent| {
                let ret = Self::handle_worker_message(event, &message_proxy, &running_msg_handler);
                if let Err(err) = ret {
                    warn!("VideoDemuxerWorker failed to process message: {err:?}");
                }
            });
        worker.set_onmessage(Some(worker_message_cb.as_ref().unchecked_ref()));
        worker_message_cb.forget();

        Ok(Self { worker, running })
    }

    fn message_proxy_send(message_proxy: &MessageProxy, msg: Message) {
        let ret = message_proxy.send(msg.clone());
        if let Err(err) = ret {
            warn!("Failed to send {msg:?}: {err:?}");
        }
    }

    fn handle_worker_message(
        event: web_sys::MessageEvent,
        message_proxy: &MessageProxy,
        running: &Rc<AtomicBool>,
    ) -> Result<(), JsValue> {
        let msg = &event.data().dyn_into::<js_sys::Object>()?;
        let msg_type = get_string(msg, "type")?;

        match msg_type.as_str() {
            "canvas_resized" => {
                let width = get_f64(msg, "width")?;
                let height = get_f64(msg, "height")?;

                Self::message_proxy_send(
                    message_proxy,
                    Message::CanvasResized {
                        width: width as _,
                        height: height as _,
                    },
                );
            }
            "decode_error" => {
                let error = js_sys::Reflect::get(msg, &"error".into())?;
                warn!("Decode error: {error:?}");

                running.store(false, Ordering::Relaxed);

                Self::message_proxy_send(message_proxy, Message::VideoDecodeError);
            }
            _ => {
                warn!("Got unknown message type: {msg_type:?}");
            }
        }

        Ok(())
    }

    pub fn start(
        &self,
        canvas: web_sys::OffscreenCanvas,
        decoder_config: VideoDecoderConfig,
    ) -> Result<(), JsValue> {
        let canvas_param = canvas.into();

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", Self::TYPE_START.into())?;
        reflect::set_ref(&msg, "canvas", &canvas_param)?;
        reflect::set(&msg, "codec", decoder_config.webcodec.into())?;

        let hardware_acceleration = match decoder_config.hardware_acceleration {
            web_sys::HardwareAcceleration::PreferHardware => true,
            web_sys::HardwareAcceleration::PreferSoftware => false,
            _ => {
                error!(
                    "Unexpected hardware acceperation {:?}",
                    decoder_config.hardware_acceleration
                );
                return Err(JsError::new("Invalid hardware acceleration setting").into());
            }
        };

        reflect::set(&msg, "hardware_acceleration", hardware_acceleration.into())?;

        let transfer = js_sys::Array::of1(&canvas_param);
        self.worker
            .post_message_with_transfer(&msg, &transfer.into())?;

        Ok(())
    }

    pub fn send_frame(&self, frame: js_sys::ArrayBuffer) -> Result<(), JsValue> {
        if !self.running.load(Ordering::Relaxed) {
            return Ok(());
        }

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", Self::TYPE_FRAME.into())?;
        reflect::set_ref(&msg, "frame", &frame)?;

        let transfer = js_sys::Array::of1(&frame);

        self.worker
            .post_message_with_transfer(&msg, &transfer.into())?;

        Ok(())
    }
}
