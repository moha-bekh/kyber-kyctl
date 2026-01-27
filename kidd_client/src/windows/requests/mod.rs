// Project Kyber: mod.rs
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

mod get_version;
mod plug_monitor;

use std::slice;

pub(crate) use get_version::send_get_version;
pub(crate) use plug_monitor::send_plug_monitor;

unsafe fn serialize<T>(data: &T) -> &[u8] {
    slice::from_raw_parts(data as *const _ as *const _, std::mem::size_of::<T>())
}

unsafe fn serialize_array<T>(data: &[T]) -> &[u8] {
    slice::from_raw_parts(data as *const _ as *const _, std::mem::size_of_val(data))
}
