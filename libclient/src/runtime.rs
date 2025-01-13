use std::future::Future;

#[cfg(target_family = "wasm")]
#[allow(unused)]
pub use kywasmtime::{
    interval, interval_at, sleep, sleep_until, timeout, timeout_at, Instant, SystemTime, UNIX_EPOCH,
};
#[cfg(target_family = "wasm")]
#[allow(unused)]
pub use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
#[allow(unused)]
pub use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(not(target_family = "wasm"))]
#[allow(unused)]
pub use tokio::time::{
    interval, interval_at, sleep, sleep_until, timeout, timeout_at, Duration, Instant,
};

#[cfg(not(target_family = "wasm"))]
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future);
}

#[cfg(target_family = "wasm")]
pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    wasm_bindgen_futures::spawn_local(future);
}
