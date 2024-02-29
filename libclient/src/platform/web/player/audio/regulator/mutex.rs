use wasm_bindgen::prelude::*;

#[wasm_bindgen(raw_module = "./mutex.js")]
extern "C" {
    pub type JSMutex;

    #[wasm_bindgen(constructor)]
    pub fn new(sabInt32Array: js_sys::Int32Array) -> JSMutex;

    #[wasm_bindgen(method)]
    pub fn lock(this: &JSMutex);

    #[wasm_bindgen(method)]
    pub fn unlock(this: &JSMutex);
}
