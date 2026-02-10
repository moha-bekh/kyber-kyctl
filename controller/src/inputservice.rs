// Project Kyber: inputservice.rs
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

use std::sync::Arc;

use log::{debug, info, warn};

use kynputservice_types::{Command, Event, HostConfig, ServiceConfig};
use libkypc::{
    process::{Process, ProcessFactory},
    CommandSender, Commander, EventReceiver,
};

use crate::error::{Error, Result};
use crate::utils;

struct Inner {
    child: Process,
    commander: Commander<Command, Event>,
    sender: CommandSender<Command, Event>,
    _receiver: EventReceiver<Event>,
}

pub(crate) struct Handler {
    inner: Option<Inner>,
}

impl Handler {
    pub(crate) fn new() -> Self {
        Self { inner: None }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    async fn spawn_process(process_factory: &Arc<ProcessFactory>) -> Result<Inner> {
        debug!("Run Input Server");

        let exe_path = utils::get_executable_path("kynputserver");

        let (child, mut commander): (_, Commander<Command, Event>) =
            process_factory.spawn(exe_path, None).await?;
        let (sender, receiver) = commander.get_sender_receiver()?;

        Ok(Inner {
            child,
            commander,
            sender,
            _receiver: receiver,
        })
    }

    pub(crate) async fn start_tcp(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        host_config: HostConfig,
        service_config: ServiceConfig,
    ) -> Result<Option<Event>> {
        if self.inner.is_some() {
            return Err(Error::StreamerStarted);
        }

        let mut inner = Self::spawn_process(process_factory).await?;
        let evt = inner
            .sender
            .send(Command::StartTcp {
                host_config,
                service_config,
            })
            .await?;
        self.inner = Some(inner);

        Ok(evt)
    }

    pub(crate) async fn start_kymux(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        host_config: HostConfig,
        service_config: ServiceConfig,
        uri: String,
    ) -> Result<Option<Event>> {
        if self.inner.is_some() {
            return Err(Error::StreamerStarted);
        }

        let mut inner = Self::spawn_process(process_factory).await?;
        let evt = inner
            .sender
            .send(Command::StartKymux {
                host_config,
                service_config,
                uri,
            })
            .await?;
        self.inner = Some(inner);

        Ok(evt)
    }

    pub(crate) async fn update_host_config(&mut self, host_config: HostConfig) -> Result<()> {
        if let Some(inner) = self.inner.as_mut() {
            inner
                .sender
                .send(Command::UpdateHostConfig { host_config })
                .await?;
        }

        Ok(())
    }

    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(mut inner) = self.inner.take() else {
            return Err(Error::StreamerStopped);
        };

        let ret = inner.sender.send(Command::Stop(0)).await;
        if let Err(err) = ret {
            warn!("Stop request send failed: {err:?}");
        }

        debug!("Stop child");
        inner.child.stop().await?;
        debug!("Stop child: Done");

        debug!("Stop commander");
        inner.commander.stop().await?;
        debug!("Stop commander: Done");

        info!("Input server exited");

        Ok(())
    }
}
