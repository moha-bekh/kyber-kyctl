// Project Kyber: dom.rs
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
