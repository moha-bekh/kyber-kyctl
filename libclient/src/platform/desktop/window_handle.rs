// Project Kyber: window_handle.rs
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

use raw_window_handle::RawWindowHandle;

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum WindowHandle {
    #[cfg(target_os = "android")]
    Android(u64),
    #[cfg(target_os = "linux")]
    Xlib(libc::c_ulong),
    #[cfg(target_os = "macos")]
    AppKit(u64),
    #[cfg(target_os = "windows")]
    Win32(u64),
    #[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
    UiKit(u64),
}

impl From<RawWindowHandle> for WindowHandle {
    fn from(handle: RawWindowHandle) -> WindowHandle {
        match handle {
            #[cfg(target_os = "linux")]
            RawWindowHandle::Xlib(handle) => WindowHandle::Xlib(handle.window),
            #[cfg(target_os = "macos")]
            RawWindowHandle::AppKit(handle) => {
                let handle: u64 = handle.ns_view as u64;

                WindowHandle::AppKit(handle)
            }
            #[cfg(target_os = "windows")]
            RawWindowHandle::Win32(handle) => {
                let handle: u64 = handle.hwnd as u64;

                WindowHandle::Win32(handle)
            }
            _ => {
                panic!("Unexpected handle type");
            }
        }
    }
}
