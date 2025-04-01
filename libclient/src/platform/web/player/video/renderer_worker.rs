use kywasmtime::Instant;
use log::{info, warn};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessageEvent;

use crate::metrics::{Metric, VideoMetric, VideoMetricData};
use crate::platform::reflect;
use crate::{Message, MessageSender};

use super::cell_buf::{self, CellBufConsumer, CellBufProducer};
use super::regulator::{self, VideoRegulatorPull, VideoRegulatorPush};
use super::renderer::{RenderResult, Renderer};

const FRAME_SKIPPED_DISPLAY_THRESHOLD: usize = 30;

pub struct VideoFrameWrapper {
    pub frame: web_sys::VideoFrame,
    pub rotation: u8,
}

impl Drop for VideoFrameWrapper {
    fn drop(&mut self) {
        self.frame.close();
    }
}

struct FrameBufPush {
    producer: CellBufProducer<VideoFrameWrapper>,
    frame_skipped: usize,
}

impl FrameBufPush {
    fn new(producer: CellBufProducer<VideoFrameWrapper>) -> Self {
        Self {
            producer,
            frame_skipped: 0,
        }
    }

    fn push(&mut self, frame: VideoFrameWrapper) -> Result<(), JsValue> {
        if let Some(prev) = self.producer.replace(frame) {
            // Previous frame skipped
            if let Some(pts) = prev.frame.timestamp() {
                let js_self = js_sys::global().dyn_into::<web_sys::DedicatedWorkerGlobalScope>()?;

                let ts = Instant::now().raw_us() as f64;

                let msg = js_sys::Object::new();
                reflect::set(&msg, "type", "metric".into())?;
                reflect::set(&msg, "metric_type", "frame_skipped".into())?;
                reflect::set(&msg, "ts", ts.into())?;
                reflect::set(&msg, "pts", pts.into())?;
                js_self.post_message(&msg)?;
            }

            self.frame_skipped += 1;
            if self.frame_skipped % FRAME_SKIPPED_DISPLAY_THRESHOLD == 0 {
                info!("{} VideoFrames skipped", self.frame_skipped);
            }
        } else if self.frame_skipped > 0 {
            info!("{} VideoFrames have been skipped", self.frame_skipped);
            self.frame_skipped = 0;
        }

        Ok(())
    }
}

enum FrameSink {
    FrameBuf(FrameBufPush),
    Regulator(VideoRegulatorPush<VideoFrameWrapper>),
}

#[wasm_bindgen]
pub struct VideoRendererWorkerCtx {
    sink: Option<FrameSink>,
}

#[wasm_bindgen]
impl VideoRendererWorkerCtx {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { sink: None }
    }

    pub fn on_message(&mut self, event: web_sys::MessageEvent) -> Result<(), JsValue> {
        let msg = &event.data().dyn_into::<js_sys::Object>()?;
        let msg_type = reflect::get_string(msg, "type")?;

        match msg_type.as_str() {
            RendererWorker::TYPE_START => {
                assert!(self.sink.is_none());
                let canvas = js_sys::Reflect::get(msg, &"canvas".into())?
                    .dyn_into::<web_sys::OffscreenCanvas>()?;
                let video_buffer_ms = js_sys::Reflect::get(msg, &"video_buffer".into())?
                    .as_f64()
                    .unwrap() as u32;

                let (producer, consumer) = cell_buf::cell_buf();
                let producer = FrameBufPush::new(producer);

                let sink = if video_buffer_ms > 0 {
                    let video_buffer_us = video_buffer_ms as u64 * 1000;
                    let (reg_push, reg_pull) = regulator::create_regulator(video_buffer_us);

                    spawn_local(async move {
                        let ret = Self::forward_frames(reg_pull, producer).await;
                        if let Err(err) = ret {
                            warn!("VideoRenderer forward_frames() ended with error {err:?}");
                        }
                    });

                    FrameSink::Regulator(reg_push)
                } else {
                    FrameSink::FrameBuf(producer)
                };
                self.sink = Some(sink);

                spawn_local(async move {
                    let ret = Self::display_frames(canvas, consumer).await;
                    if let Err(err) = ret {
                        warn!("VideoRenderer display_frames() ended with error {err:?}");
                    }
                });
            }
            RendererWorker::TYPE_FRAME => {
                let sink = self.sink.as_mut().expect("Frame sink not initialized");

                let frame = js_sys::Reflect::get(msg, &"frame".into())?
                    .dyn_into::<web_sys::VideoFrame>()?;
                let rotation = reflect::get_f64(msg, "rotation")? as u8;
                let frame_w = VideoFrameWrapper { frame, rotation };

                match sink {
                    FrameSink::FrameBuf(ref mut sink) => sink.push(frame_w)?,
                    FrameSink::Regulator(ref mut sink) => {
                        let recv_date = reflect::get_f64(msg, "recv_date")? as u64;
                        let pts = frame_w.frame.timestamp().expect("No pts") as u64;
                        if let Err(err) = sink.push(frame_w, recv_date, pts) {
                            warn!("Error pushing frame: {err:?}");
                        }
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

    async fn forward_frames(
        mut reg_pull: VideoRegulatorPull<VideoFrameWrapper>,
        mut producer: FrameBufPush,
    ) -> Result<(), JsValue> {
        while let Some(frame) = reg_pull.pull().await {
            producer.push(frame)?;
        }

        Ok(())
    }

    async fn display_frames(
        canvas: web_sys::OffscreenCanvas,
        consumer: CellBufConsumer<VideoFrameWrapper>,
    ) -> Result<(), JsValue> {
        let js_self = js_sys::global().dyn_into::<web_sys::DedicatedWorkerGlobalScope>()?;

        let mut first_frame = true;
        let renderer = Renderer::new(canvas)?;

        loop {
            let frame = consumer.take().await;

            let timestamp = frame.frame.timestamp();
            let result = renderer.draw(frame)?;

            if let Some(pts) = timestamp {
                let ts = Instant::now().raw_us() as f64;

                let msg = js_sys::Object::new();
                reflect::set(&msg, "type", "metric".into())?;
                reflect::set(&msg, "metric_type", "frame_displayed".into())?;
                reflect::set(&msg, "ts", ts.into())?;
                reflect::set(&msg, "pts", pts.into())?;
                js_self.post_message(&msg)?;
            }

            match result {
                None => {}
                Some(RenderResult::CanvasResized {
                    width,
                    height,
                    frame_coded_width,
                    frame_coded_height,
                }) => {
                    let msg = js_sys::Object::new();
                    reflect::set(&msg, "type", "canvas_resized".into())?;
                    reflect::set(&msg, "height", height.into())?;
                    reflect::set(&msg, "width", width.into())?;
                    reflect::set(&msg, "frame_coded_width", frame_coded_width.into())?;
                    reflect::set(&msg, "frame_coded_height", frame_coded_height.into())?;

                    js_self.post_message(&msg)?;
                }
            }

            if first_frame {
                info!("First video frame rendered");
                first_frame = false;
            }
        }
    }
}

pub struct RendererWorker {
    worker: Rc<web_sys::Worker>,
    _message_closure: Closure<dyn FnMut(MessageEvent)>,
}

impl RendererWorker {
    const TYPE_START: &'static str = "start";
    const TYPE_FRAME: &'static str = "frame";

    pub fn create(
        message_sender: MessageSender,
    ) -> Result<(RendererWorker, RendererWorkerHandle), JsValue> {
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);

        let worker = Rc::new(web_sys::Worker::new_with_options(
            "./video_renderer_worker.js",
            &options,
        )?);

        let worker_message_cb =
            Closure::<dyn FnMut(_)>::new(move |event: web_sys::MessageEvent| {
                let ret = Self::handle_worker_message(event, &message_sender);
                if let Err(err) = ret {
                    warn!("VideoDemuxerWorker failed to process message: {err:?}");
                }
            });
        worker.set_onmessage(Some(worker_message_cb.as_ref().unchecked_ref()));

        let renderer_worker = Self {
            worker: worker.clone(),
            _message_closure: worker_message_cb,
        };
        let handle = RendererWorkerHandle { worker };

        Ok((renderer_worker, handle))
    }

    fn handle_worker_message(
        event: web_sys::MessageEvent,
        message_sender: &MessageSender,
    ) -> Result<(), JsValue> {
        let msg = &event.data().dyn_into::<js_sys::Object>()?;
        let msg_type = reflect::get_string(msg, "type")?;

        match msg_type.as_str() {
            "canvas_resized" => {
                let width = reflect::get_f64(msg, "width")?;
                let height = reflect::get_f64(msg, "height")?;
                let frame_coded_width = reflect::get_f64(msg, "frame_coded_width")?;
                let frame_coded_height = reflect::get_f64(msg, "frame_coded_height")?;

                Self::message_sender_send(
                    message_sender,
                    Message::CanvasResized {
                        width: width as _,
                        height: height as _,
                        frame_coded_width: frame_coded_width as _,
                        frame_coded_height: frame_coded_height as _,
                    },
                );
            }
            "metric" => {
                let metric_type = match reflect::get_string(msg, "metric_type")?.as_str() {
                    "frame_skipped" => VideoMetricData::FrameSkipped,
                    "frame_displayed" => VideoMetricData::FrameDisplayed,
                    metric_type => {
                        return Err(JsValue::from(format!(
                            "Unexpected metric type: {metric_type}"
                        )))
                    }
                };
                let pts = reflect::get_f64(msg, "pts")? as u64;
                let ts = reflect::get_f64(msg, "ts")? as i64;

                let metric = Metric::Video(VideoMetric {
                    pts,
                    ts,
                    data: metric_type,
                });
                Self::message_sender_send(message_sender, Message::Metric(metric));
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
        video_buffer: u32,
    ) -> Result<(), JsValue> {
        let canvas_param = canvas.into();

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", Self::TYPE_START.into())?;
        reflect::set_ref(&msg, "canvas", &canvas_param)?;
        reflect::set(&msg, "video_buffer", video_buffer.into())?;

        let transfer = js_sys::Array::of1(&canvas_param);

        self.worker
            .post_message_with_transfer(&msg, &transfer.into())?;

        Ok(())
    }

    pub fn send_frame(
        &self,
        frame: web_sys::VideoFrame,
        rotation: u8,
        recv_date: i64,
    ) -> Result<(), JsValue> {
        let frame_param: JsValue = frame.into();

        let msg = js_sys::Object::new();
        reflect::set(&msg, "type", Self::TYPE_FRAME.into())?;
        reflect::set_ref(&msg, "frame", &frame_param)?;
        reflect::set(&msg, "rotation", rotation.into())?;
        let recv_date = recv_date as f64;
        reflect::set(&msg, "recv_date", recv_date.into())?;

        let transfer = js_sys::Array::of1(&frame_param);

        self.worker
            .post_message_with_transfer(&msg, &transfer.into())?;

        Ok(())
    }

    fn message_sender_send(message_sender: &MessageSender, msg: Message) {
        let ret = message_sender.send(msg.clone());
        if let Err(err) = ret {
            warn!("Failed to send {msg:?}: {err:?}");
        }
    }
}

pub(crate) struct RendererWorkerHandle {
    worker: Rc<web_sys::Worker>,
}

impl RendererWorkerHandle {
    pub(crate) fn stop(&mut self) -> Result<(), JsValue> {
        self.worker.terminate();
        info!("Video renderer worker stopped");

        Ok(())
    }
}
