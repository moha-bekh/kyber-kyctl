use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use log::{error, info, warn};
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio::sync::oneshot;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessageEvent;

use crate::platform::player::packet::buffer_to_packet;
use crate::platform::player::packet::{CodecPacket, Packet};
use crate::platform::reflect::{self, get_string};
use crate::{Message, MessageProxy};

use super::decoder::{AudioDecoder, AudioDecoderCallbacks};
use super::RingBuffer;

#[wasm_bindgen]
pub struct AudioDemuxerWorkerCtx {
    tx: mpsc::Sender<js_sys::ArrayBuffer>,
    rx: Option<mpsc::Receiver<js_sys::ArrayBuffer>>,
}

#[wasm_bindgen]
impl AudioDemuxerWorkerCtx {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        // - 1 frame is required to configure the AudioDecoder.
        // - Assume we get 20 ms audio chunks. Bufferize 100 ms at most.
        let (tx, rx) = mpsc::channel(5);

        Self { tx, rx: Some(rx) }
    }

    pub async fn on_message(
        &mut self,
        event: web_sys::MessageEvent,
    ) -> std::result::Result<(), JsValue> {
        let msg = &event.data().dyn_into::<js_sys::Object>()?;
        let msg_type = get_string(msg, "type")?;

        match msg_type.as_str() {
            DemuxerWorker::TYPE_START => {
                let sab = js_sys::Reflect::get(msg, &"sab".into())?
                    .dyn_into::<js_sys::SharedArrayBuffer>()?;

                let Some(rx) = self.rx.take() else {
                    warn!("AudioDemuxerWorker has already received Start message");
                    return Err(JsError::new("Start message already received").into());
                };

                spawn_local(async move {
                    let ret = Self::demux_process_loop(rx, sab).await;
                    if let Err(err) = ret {
                        error!("Audio demux loop ended with error {err:?}");

                        let ret = Self::forward_error_to_parent("decode_error", err);
                        if let Err(err) = &ret {
                            warn!("Failed to forward error to parent: {err:?}");
                        }
                    }
                });
            }
            DemuxerWorker::TYPE_STOP => {
                info!("Audio demuxer stop requested");

                let js_self =
                    js_sys::eval("self")?.dyn_into::<web_sys::DedicatedWorkerGlobalScope>()?;
                let msg = js_sys::Object::new();
                reflect::set(&msg, "type", "stopped".into())?;

                js_self.post_message(&msg)?;
                js_self.close();
            }
            DemuxerWorker::TYPE_FRAME => {
                let frame = js_sys::Reflect::get(msg, &"frame".into())?
                    .dyn_into::<js_sys::ArrayBuffer>()?;

                match self.tx.try_send(frame) {
                    Ok(_) => {}
                    Err(TrySendError::Closed(_)) => {
                        warn!("Failed to forward AudioFrame to demuxer task: channel closed");
                    }
                    Err(TrySendError::Full(_)) => {
                        warn!("AudioFrame queue full: frame dropped");
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
        sab: js_sys::SharedArrayBuffer,
    ) -> Result<(), JsValue> {
        let ringbuffer = RingBuffer::from(sab);

        let mut audio_decoder: Option<AudioDecoder> = None;

        let mut cbs = Some(AudioDecoderCallbacks {
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

                ringbuffer.write(frame)?;

                Ok(())
            }),
            error: Box::new(move |error| {
                info!("audio decoding error: {error:?}");

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
                    assert!(audio_decoder.is_some());
                    if let Some(audio_decoder) = &mut audio_decoder {
                        audio_decoder.send_packet(packet)?;
                    } else {
                        error!("Audio decoder not set");
                        return Err(JsError::new("Audio decoder not set").into());
                    }
                }
                Packet::Codec(CodecPacket { rotation: _ }) => {
                    if audio_decoder.is_none() {
                        info!("Audio codec packet");

                        let Some(cbs) = cbs.take() else {
                            error!("Callbacks already consumed");
                            return Err(JsError::new("Callbacks already consumed").into());
                        };

                        audio_decoder = Some(AudioDecoder::new(cbs)?);
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
    stop_tx: Rc<RefCell<Option<oneshot::Sender<()>>>>,
    _message_closure: Closure<dyn FnMut(MessageEvent)>,
}

impl DemuxerWorker {
    const TYPE_START: &str = "start";
    const TYPE_STOP: &str = "stop";
    const TYPE_FRAME: &str = "frame";

    pub fn new(message_proxy: MessageProxy) -> Result<DemuxerWorker, JsValue> {
        let worker = web_sys::Worker::new_with_options(
            "./audio_demuxer_worker.js",
            web_sys::WorkerOptions::new().type_(web_sys::WorkerType::Module),
        )?;

        let running = Rc::new(AtomicBool::new(true));
        let stop_tx = Rc::new(RefCell::new(None));

        // Get messages from worker
        let running_msg_handler = running.clone();
        let worker_cb_stop_tx = stop_tx.clone();

        let worker_message_cb =
            Closure::<dyn FnMut(_)>::new(move |event: web_sys::MessageEvent| {
                let ret = Self::handle_worker_message(
                    event,
                    &message_proxy,
                    &running_msg_handler,
                    &worker_cb_stop_tx,
                );
                if let Err(err) = ret {
                    warn!("AudioDemuxerWorker failed to process message: {err:?}");
                }
            });
        worker.set_onmessage(Some(worker_message_cb.as_ref().unchecked_ref()));

        Ok(Self {
            worker,
            running,
            stop_tx,
            _message_closure: worker_message_cb,
        })
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
        stop_tx: &Rc<RefCell<Option<oneshot::Sender<()>>>>,
    ) -> Result<(), JsValue> {
        let msg = &event.data().dyn_into::<js_sys::Object>()?;
        let msg_type = get_string(msg, "type")?;

        match msg_type.as_str() {
            "decode_error" => {
                let error = js_sys::Reflect::get(msg, &"error".into())?;
                warn!("Audio decode error: {error:?}");

                running.store(false, Ordering::Relaxed);

                Self::message_proxy_send(message_proxy, Message::AudioDecodeError);
            }
            "stopped" => {
                let Some(stop_tx) = stop_tx.borrow_mut().take() else {
                    warn!("Renderer worker stop_tx None");
                    return Ok(());
                };

                let _ = stop_tx.send(());
            }
            _ => {
                warn!("Got unknown message type: {msg_type:?}");
            }
        }

        Ok(())
    }

    pub fn start(&self, sab: js_sys::SharedArrayBuffer) -> Result<(), JsValue> {
        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", Self::TYPE_START.into())?;
        reflect::set(&msg, "sab", sab.into())?;

        self.worker.post_message(&msg)?;

        Ok(())
    }

    pub async fn stop(&self) -> Result<(), JsValue> {
        self.running.store(false, Ordering::Relaxed);

        let (stop_tx, stop_rx) = oneshot::channel();

        {
            let mut self_stop_tx = self.stop_tx.borrow_mut();
            *self_stop_tx = Some(stop_tx);
        }

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", Self::TYPE_STOP.into())?;

        self.worker.post_message(&msg)?;

        let _ = stop_rx.await;
        info!("Audio demuxer worker stopped");

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
