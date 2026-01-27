// Project Kyber: reflect.rs
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

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

#[allow(dead_code)]
pub(crate) fn get_bool(obj: &Object, name: &str) -> Result<bool, JsValue> {
    match Reflect::get(obj, &name.into())?.as_bool() {
        Some(b) => Ok(b),
        None => Err(JsError::new(&format!("{name} attribute isn't a boolean")).into()),
    }
}

pub(crate) fn get_f64(obj: &Object, name: &str) -> Result<f64, JsValue> {
    match Reflect::get(obj, &name.into())?.as_f64() {
        Some(f) => Ok(f),
        None => Err(JsError::new(&format!("{name} attribute isn't a f64")).into()),
    }
}

pub(crate) fn get_string(obj: &Object, name: &str) -> Result<String, JsValue> {
    match Reflect::get(obj, &name.into())?.as_string() {
        Some(s) => Ok(s),
        None => Err(JsError::new(&format!("{name} attribute isn't a string")).into()),
    }
}

pub(crate) fn set(obj: &Object, key: &str, value: JsValue) -> Result<(), JsValue> {
    match Reflect::set(obj, &key.into(), &value) {
        Ok(true) => Ok(()),
        Ok(false) => {
            Err(JsError::new(&format!("Set attribute {key}: set() returned false")).into())
        }
        Err(err) => Err(JsError::new(&format!("Fail to set attribute {key}: {err:?}")).into()),
    }
}

pub(crate) fn set_ref(obj: &Object, key: &str, value: &JsValue) -> Result<(), JsValue> {
    match Reflect::set(obj, &key.into(), value) {
        Ok(true) => Ok(()),
        Ok(false) => {
            Err(JsError::new(&format!("Set attribute {key}: set() returned false")).into())
        }
        Err(err) => Err(JsError::new(&format!("Fail to set attribute {key}: {err:?}")).into()),
    }
}
