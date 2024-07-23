use std::f64::consts::PI;

use wasm_bindgen::prelude::*;

use super::renderer_worker::VideoFrameWrapper;

pub(super) enum RenderResult {
    CanvasResized {
        width: u32,
        height: u32,
        frame_coded_width: u32,
        frame_coded_height: u32,
    },
}

pub(super) struct Renderer {
    canvas: web_sys::OffscreenCanvas,
    ctx: web_sys::OffscreenCanvasRenderingContext2d,
}

impl Renderer {
    pub(super) fn new(canvas: web_sys::OffscreenCanvas) -> Result<Self, JsValue> {
        let ctx = canvas
            .get_context("2d")?
            .ok_or(JsValue::from(js_sys::Error::new("No 2D context")))?
            .dyn_into::<web_sys::OffscreenCanvasRenderingContext2d>()?;

        Ok(Self { canvas, ctx })
    }

    pub(super) fn draw(&self, frame: VideoFrameWrapper) -> Result<Option<RenderResult>, JsValue> {
        let frame_width = frame.frame.display_width();
        let frame_height = frame.frame.display_height();

        let (display_width, display_height, angle) = match frame.rotation {
            0 /* KYBER_ROTATION_IDENTITY */ => (frame_width, frame_height, None),
            1 /* KYBER_ROTATION_ROTATE90 */ => (frame_height, frame_width, Some(90)),
            2 /* KYBER_ROTATION_ROTATE180 */ => (frame_width, frame_height, Some(180)),
            3 /* KYBER_ROTATION_ROTATE270 */ => (frame_height, frame_width, Some(270)),
            _ => (frame_width, frame_height, None)
        };

        let render_result =
            if self.canvas.width() != display_width || self.canvas.height() != display_height {
                self.canvas.set_width(display_width);
                self.canvas.set_height(display_height);

                Some(RenderResult::CanvasResized {
                    width: display_width,
                    height: display_height,
                    frame_coded_width: frame.frame.coded_width(),
                    frame_coded_height: frame.frame.coded_height(),
                })
            } else {
                None
            };

        if let Some(angle) = angle {
            let dx = (display_width / 2) as f64;
            let dy = (display_height / 2) as f64;
            let rotate = angle as f64 * PI / 180.0;

            self.ctx.save();

            self.ctx.translate(dx, dy)?;
            self.ctx.rotate(rotate)?;

            self.ctx.draw_image_with_video_frame_and_dw_and_dh(
                &frame.frame,
                -(frame_width as f64) / 2.0,
                -(frame_height as f64) / 2.0,
                frame_width as f64,
                frame_height as f64,
            )?;

            self.ctx.restore();
        } else {
            self.ctx.draw_image_with_video_frame_and_dw_and_dh(
                &frame.frame,
                0.0,
                0.0,
                frame_width as f64,
                frame_height as f64,
            )?;
        };

        frame.frame.close();

        Ok(render_result)
    }
}
