// Project Kyber: utils.rs
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

use std::path::PathBuf;

use crate::host_events;

pub(crate) fn generate_session_id() -> u32 {
    use rand::prelude::*;
    rand::rng().random()
}

pub(crate) fn get_resource_path(resource_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(resource_name);

        path
    } else {
        PathBuf::from(resource_name)
    }
}

// Windows requires a fullpath.
// Unix platforms must have $PATH correctly configured.
#[cfg(target_os = "windows")]
pub fn get_executable_path(exe_name: &str) -> PathBuf {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        path.push(exe_name);

        path
    } else {
        PathBuf::from(exe_name)
    }
}

#[cfg(not(any(target_os = "windows")))]
pub fn get_executable_path(exe_name: &str) -> PathBuf {
    PathBuf::from(exe_name)
}

pub(crate) trait TypeConvert<T> {
    fn convert(&self) -> T;
}

impl<F, T> TypeConvert<Vec<T>> for Vec<F>
where
    F: TypeConvert<T>,
{
    fn convert(&self) -> Vec<T> {
        self.iter().map(|i| i.convert()).collect()
    }
}

impl TypeConvert<host_events::Display> for kyavservice_types::Display {
    fn convert(&self) -> host_events::Display {
        host_events::Display {
            id: self.id,
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            x: self.x,
            y: self.y,
        }
    }
}

impl TypeConvert<kynputservice_types::Display> for kyavservice_types::Display {
    fn convert(&self) -> kynputservice_types::Display {
        kynputservice_types::Display {
            id: self.id,
            width: self.width,
            height: self.height,
            x: self.x,
            y: self.y,
        }
    }
}

pub(crate) struct Task(tokio::task::JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Task {
    pub fn spawn<F>(f: F) -> Self
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        Self(tokio::spawn(f))
    }

    pub fn spawn_local<F>(f: F) -> Self
    where
        F: std::future::Future<Output = ()> + 'static,
    {
        Self(tokio::task::spawn_local(f))
    }
}
