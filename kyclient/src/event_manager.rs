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

pub trait EventSink {
    fn on_event(&self, event: &Event);
}

#[cfg(target_arch = "wasm32")]
type BoxedEventSink = Box<dyn EventSink>;

#[cfg(not(target_arch = "wasm32"))]
type BoxedEventSink = Box<dyn EventSink + Send>;

#[derive(Default)]
struct Inner {
    sinks: Vec<BoxedEventSink>,
}

#[derive(Default)]
pub struct EventManager {
    #[cfg(target_arch = "wasm32")]
    inner: std::rc::Rc<std::cell::RefCell<Inner>>,

    #[cfg(not(target_arch = "wasm32"))]
    inner: std::sync::Arc<std::sync::Mutex<Inner>>,
}

impl EventManager {
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }

    pub fn add_event_sink(&self, sink: BoxedEventSink) {
        #[cfg(not(target_arch = "wasm32"))]
        let Ok(mut inner) = self.inner.lock() else {
            log::warn!("Failed to lock EventManager::inner");
            return;
        };

        #[cfg(target_arch = "wasm32")]
        let mut inner = self.inner.borrow_mut();

        inner.sinks.push(sink);
    }

    pub fn send_event(&self, event: Event) {
        #[cfg(not(target_arch = "wasm32"))]
        let Ok(inner) = self.inner.lock() else {
            log::warn!("Failed to lock EventManager::inner");
            return;
        };

        #[cfg(target_arch = "wasm32")]
        let inner = self.inner.borrow();

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
