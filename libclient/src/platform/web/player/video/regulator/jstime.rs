use web_sys::wasm_bindgen::JsCast;

pub fn now_us() -> u64 {
    let perf = js_sys::Reflect::get(&js_sys::global(), &"performance".into())
        .unwrap()
        .dyn_into::<web_sys::Performance>()
        .expect("No performance field");
    ((perf.time_origin() + perf.now()) * 1000.0) as u64
}
