// Project Kyber: plug_monitor.rs
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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use kidd_client::MonitorMode;

fn main() {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        println!("Exit requested");
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    println!("Plug monitor");

    let modes = [
        MonitorMode {
            width: 1920,
            height: 1080,
            vsync: 60,
        },
        MonitorMode {
            width: 3840,
            height: 2160,
            vsync: 60,
        },
        MonitorMode {
            width: 2560,
            height: 1440,
            vsync: 60,
        },
        MonitorMode {
            width: 800,
            height: 600,
            vsync: 60,
        },
    ];

    let device_interface = kidd_client::Driver::open().unwrap();
    device_interface.plug_monitor(&modes, 0).unwrap();

    println!("Monitor plugged");

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    println!("Exit sample");
}
