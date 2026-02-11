// Project Kyber: servicecontrol_forwarder.rs
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

use std::sync::mpsc::{self, TryRecvError};

use windows_service::service::SessionChangeParam;
use windows_sys::Win32::Foundation::HANDLE;

use super::{Error, Event, Result};

pub(crate) struct Sender {
    tx: mpsc::Sender<SessionChangeParam>,
    event: Event,
}

impl Sender {
    pub(crate) fn send(&self, param: SessionChangeParam) -> Result<()> {
        self.tx.send(param).map_err(|_| Error::MpscChannelClosed {
            channel_name: self.event.name().into(),
        })?;

        self.event.set()?;

        Ok(())
    }
}

pub(crate) struct Receiver {
    rx: mpsc::Receiver<SessionChangeParam>,
    event: Event,
}

impl Receiver {
    pub(crate) fn handle(&self) -> HANDLE {
        self.event.handle()
    }

    pub(crate) fn recv(&self) -> Result<SessionChangeParam> {
        self.rx.try_recv().map_err(|err| match err {
            TryRecvError::Empty => Error::MpscChannelEmpty,
            TryRecvError::Disconnected => Error::MpscChannelClosed {
                channel_name: self.event.name().into(),
            },
        })
    }
}

pub(crate) fn create() -> Result<(Sender, Receiver)> {
    let event = Event::new("SessionNotif", false)?;
    let (tx, rx) = mpsc::channel();

    Ok((
        Sender {
            tx,
            event: event.clone(),
        },
        Receiver { rx, event },
    ))
}
