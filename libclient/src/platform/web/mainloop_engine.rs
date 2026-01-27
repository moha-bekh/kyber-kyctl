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

use log::{info, warn};
use tokio::sync::oneshot;

use crate::MainLoop;

pub(crate) struct MainLoopEngine {
    stop_tx: oneshot::Sender<()>,
    stopped_rx: oneshot::Receiver<()>,
}

impl MainLoopEngine {
    pub(crate) fn new(main_loop: MainLoop) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (stopped_tx, stopped_rx) = oneshot::channel();

        wasm_bindgen_futures::spawn_local(Self::engine_entry(stop_rx, stopped_tx, main_loop));

        Self {
            stop_tx,
            stopped_rx,
        }
    }

    async fn engine_entry(
        stop_rx: oneshot::Receiver<()>,
        stopped_tx: oneshot::Sender<()>,
        main_loop: MainLoop,
    ) {
        tokio::select! {
            _ = stop_rx => {
                info!("Main loop stop requested");
            }
            _ = main_loop.run() => {
                info!("Main loop exited");
            }
        }

        let _ = stopped_tx.send(());
    }

    pub(crate) async fn stop(self) {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("Failed to request main loop stop");
        }

        let ret = self.stopped_rx.await;
        if let Err(err) = ret {
            warn!("Failed to join main loop: {err:?}");
        }
    }
}
