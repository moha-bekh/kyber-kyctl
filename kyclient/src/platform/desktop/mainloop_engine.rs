// Project Kyber: mainloop_engine.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

use crate::MainLoop;
use std::sync::Mutex;

pub(crate) struct MainLoopEngine {
    rt: Mutex<Option<tokio::runtime::Runtime>>,
}

impl MainLoopEngine {
    pub(crate) fn new(main_loop: MainLoop) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("EventLoop")
            .enable_io()
            .enable_time()
            .build()
            .unwrap();

        rt.spawn(main_loop.run());

        Self {
            rt: Mutex::new(Some(rt)),
        }
    }

    pub(crate) fn stop(&self) {
        // Dropping the runtime cancels its futures
        *self.rt.lock().unwrap() = None;
    }
}
