use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

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
