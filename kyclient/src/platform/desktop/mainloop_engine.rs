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

use crate::{MainLoop, runtime::Task};
use std::sync::Mutex;

struct State {
    task: Task,
    rt: Option<tokio::runtime::Runtime>,
}

pub(crate) struct MainLoopEngine {
    state: Mutex<State>,
}

impl MainLoopEngine {
    pub(crate) fn new(main_loop: MainLoop) -> Self {
        let mut rt = None;
        let handle;

        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => {
                handle = h;
                &handle
            }
            Err(_) => {
                let rt = rt.insert(
                    tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(1)
                        .thread_name("EventLoop")
                        .enable_io()
                        .enable_time()
                        .build()
                        .unwrap(),
                );
                rt.handle()
            }
        };

        let task = Task::spawn_in(handle, main_loop.run());

        Self {
            state: Mutex::new(State { task, rt }),
        }
    }

    pub(crate) fn stop(&self) {
        let state = &mut *self.state.lock().unwrap();
        state.task.abort();
        state.rt = None;
    }
}
