use std::sync::Mutex;
use std::thread::{self, JoinHandle};

use log::{info, warn};
use tokio::sync::oneshot;

use crate::{Error, MainLoop, Result};

struct Inner {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,
}

pub(crate) struct MainLoopEngine {
    inner: Mutex<Option<Inner>>,
}

impl MainLoopEngine {
    pub(crate) fn new(main_loop: MainLoop) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join_handle = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("EventLoop")
                .enable_io()
                .enable_time()
                .build()
                .unwrap();

            rt.block_on(Self::engine_entry(stop_rx, main_loop));
        });

        let inner = Inner {
            stop_tx,
            join_handle,
        };

        Self {
            inner: Mutex::new(Some(inner)),
        }
    }

    async fn engine_entry(stop_rx: oneshot::Receiver<()>, main_loop: MainLoop) {
        tokio::select! {
            _ = stop_rx => {
                info!("Main loop stopped");
            }
            _ = main_loop.run() => {}
        }
    }

    pub(crate) fn stop(&self) -> Result<()> {
        let mut inner = self.inner.lock()?;
        let Some(inner) = inner.take() else {
            warn!("MainloopEngine already stopped");
            return Err(Error::Rejected);
        };

        let ret = inner.stop_tx.send(());
        if ret.is_err() {
            warn!("Failed to request main loop stop");
        }

        let ret = inner.join_handle.join();
        if let Err(err) = ret {
            warn!("Failed to join main loop: {err:?}");
        }

        Ok(())
    }
}
