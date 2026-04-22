// Project Kyber: inputservice.rs
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

use log::{debug, warn};

use kynputservice_types::{Command, HostConfig, IpcApi, Response, ServiceConfig};
use libkypc::{
    process::{Process, ProcessFactory},
    Commander,
};

use crate::error::Result;
use crate::utils;

pub(crate) struct Handler {
    _child: Process,
    commander: Commander<IpcApi>,
}

impl Handler {
    async fn spawn_process(process_factory: &Arc<ProcessFactory>) -> Result<Self> {
        debug!("Run Input Server");

        let exe_path = utils::get_executable_path("kynputserver");

        let (_child, commander) = process_factory.spawn(exe_path, &[]).await?;

        Ok(Self { _child, commander })
    }

    pub(crate) async fn start_tcp(
        process_factory: &Arc<ProcessFactory>,
        host_config: HostConfig,
        service_config: ServiceConfig,
    ) -> Result<(Self, Response)> {
        let mut this = Self::spawn_process(process_factory).await?;
        let res = this
            .commander
            .commands
            .send(Command::StartTcp {
                host_config,
                service_config,
            })
            .await?
            .unwrap();

        Ok((this, res))
    }

    pub(crate) async fn start_kymux(
        process_factory: &Arc<ProcessFactory>,
        host_config: HostConfig,
        service_config: ServiceConfig,
        uri: String,
    ) -> Result<(Self, Response)> {
        let mut this = Self::spawn_process(process_factory).await?;
        let res = this
            .commander
            .commands
            .send(Command::StartKymux {
                host_config,
                service_config,
                uri,
            })
            .await?
            .unwrap();

        Ok((this, res))
    }

    pub(crate) async fn update_host_config(&mut self, host_config: HostConfig) -> Result<()> {
        self.commander
            .commands
            .send(Command::UpdateHostConfig { host_config })
            .await??;

        Ok(())
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        let ret = self.commander.commands.send(Command::Stop(0)).await;
        if let Err(err) = ret {
            warn!("Stop request send failed: {err:?}");
        }

        Ok(())
    }
}
