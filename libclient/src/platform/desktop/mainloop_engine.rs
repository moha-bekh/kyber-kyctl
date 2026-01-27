// Project Kyber: mainloop_engine.rs
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
