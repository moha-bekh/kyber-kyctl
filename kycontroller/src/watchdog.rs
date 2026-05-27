// Project Kyber: watchdog.rs
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

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::{Result, StateMachineProxy};

pub(crate) struct Watchdog {
    avservice: avservice::Handler,
}

impl Watchdog {
    pub(crate) async fn new(
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(Self, Vec<kyavservice_types::Display>)> {
        // Watchdog is global, not tied to any user session
        let mut avservice = avservice::Handler::new(
            avservice::HandlerType::EnumerateDisplays,
            None,
            None,
            statemachine_proxy.clone(),
        );

        let displays = avservice.enumerate_displays(process_factory).await?;

        Ok((Self { avservice }, displays))
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        self.avservice.stop().await?;

        Ok(())
    }
}
