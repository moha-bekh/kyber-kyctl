// Project Kyber: event_manager.rs
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

use super::enums::Event;

pub trait EventSink: crate::util::KySend {
    fn on_event(&self, event: &Event);
}

type BoxedEventSink = Box<dyn EventSink>;

#[derive(Default)]
struct Inner {
    sinks: Vec<BoxedEventSink>,
}

#[derive(Default)]
pub struct EventManager {
    inner: std::sync::Arc<std::sync::Mutex<Inner>>,
}

impl EventManager {
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }

    pub fn add_event_sink(&self, sink: BoxedEventSink) {
        let mut inner = self.inner.lock().unwrap();
        inner.sinks.push(sink);
    }

    pub fn send_event(&self, event: Event) {
        let inner = self.inner.lock().unwrap();

        for sink in inner.sinks.iter() {
            sink.on_event(&event);
        }
    }
}

impl Clone for EventManager {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
