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
pub use tokio::task::JoinHandle;

#[cfg(target_family = "wasm")]
pub struct JoinHandle<T> {
    abort_handle: futures::future::AbortHandle,
    _marker: std::marker::PhantomData<T>,
}

#[cfg(target_family = "wasm")]
impl<T> JoinHandle<T> {
    pub fn abort(&self) {
        self.abort_handle.abort();
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn spawn<F>(future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(future)
}

#[cfg(target_family = "wasm")]
pub fn spawn<F>(future: F) -> JoinHandle<()>
where
    F: Future<Output = ()> + 'static,
{
    let (abort_handle, abort_registration) = futures::future::AbortHandle::new_pair();
    let abortable_future = futures::future::Abortable::new(future, abort_registration);

    wasm_bindgen_futures::spawn_local(async {
        let _ = abortable_future.await;
    });

    JoinHandle {
        abort_handle,
        _marker: std::marker::PhantomData,
    }
}
