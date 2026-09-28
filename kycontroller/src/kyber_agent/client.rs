// Project Kyber: kyber_agent/client.rs
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

use actix_web::web;
use kyagent::{ActiveSession, Config, Heartbeat, HeartbeatSource, Machine};
use log::warn;

use crate::controller::Controller;
use crate::kyber_agent::Reporter;

pub(crate) struct KyberAgentTask(Heartbeat);

impl KyberAgentTask {
    pub(crate) fn new(config: Config, ctrl: web::Data<Controller>) -> Self {
        let (reporter, events) = kyagent::channel();
        let machine = Machine {
            version: env!("VERGEN_GIT_DESCRIBE").to_owned(),
            port: ctrl.config.port(),
        };
        ctrl.set_kyhub_reporter(Reporter(reporter));

        Self(Heartbeat::start(
            config,
            machine,
            ControllerSessions { ctrl },
            events,
        ))
    }

    /// Tell kyhub the machine stops on purpose. kyagent bounds the wait.
    pub(crate) async fn stop(self) {
        self.0.stop().await;
    }
}

/// Gives kyagent the active sessions.
struct ControllerSessions {
    ctrl: web::Data<Controller>,
}

impl HeartbeatSource for ControllerSessions {
    async fn active_sessions(&self) -> Vec<ActiveSession> {
        self.ctrl
            .sessions
            .lock()
            .expect("sessions lock poisoned")
            .iter()
            .filter_map(|(session_id, s)| {
                let started_at = s.started_at?;
                let Some(user_sub) = s.kyhub_user_sub() else {
                    warn!("kyhub heartbeat: dropping session {session_id}, user_sub is not a UUID");
                    return None;
                };
                Some(ActiveSession {
                    kycontroller_session_id: *session_id,
                    user_sub: user_sub.to_owned(),
                    started_at,
                })
            })
            .collect()
    }
}
