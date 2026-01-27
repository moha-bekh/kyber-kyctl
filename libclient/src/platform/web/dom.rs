use log::error;

use wasm_bindgen::prelude::*;

use crate::{Error, Result};

pub(crate) fn window() -> Result<web_sys::Window> {
    let Some(window) = web_sys::window() else {
        error!("Couldn't find DOM's window");
        return Err(Error::JsDomInvalid);
    };

    Ok(window)
}

pub(crate) fn document() -> Result<web_sys::Document> {
    let window = window()?;

    let Some(document) = window.document() else {
        error!("Couldn't find DOM's document");
        return Err(Error::JsDomInvalid);
    };

    Ok(document)
}

pub(crate) fn get_canvas(canvas: &str) -> Result<web_sys::HtmlCanvasElement> {
    document()?
        .get_element_by_id(canvas)
        .ok_or(Error::JsElementNotFound {
            name: canvas.into(),
        })?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| Error::JsCastFailed {
            to: "HtmlCanvasElement".into(),
        })
}
