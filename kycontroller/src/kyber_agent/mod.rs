// Project Kyber: kyber_agent/mod.rs
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

use kyagent::SessionEndReason as WireEndReason;
use uuid::Uuid;

use crate::controller::{SessionEndReason, SessionStep};

/// Maps session steps to kyagent's reporter.
pub(crate) struct Reporter(kyagent::Reporter);

impl Reporter {
    /// Queues one step. kyagent drops it when the queue is full.
    pub(crate) fn report(&self, step: SessionStep, uid: Uuid, user_sub: &str) {
        match step {
            SessionStep::Started => self.0.session_started(uid, user_sub),
            SessionStep::Reconnected => self.0.session_reconnected(uid, user_sub),
            SessionStep::Ended(reason) => {
                self.0.session_ended(uid, user_sub, wire_end_reason(reason))
            }
        }
    }
}

fn wire_end_reason(reason: SessionEndReason) -> WireEndReason {
    match reason {
        SessionEndReason::ConnectionLost => WireEndReason::ConnectionLost,
        SessionEndReason::UserLogout => WireEndReason::UserLogout,
        SessionEndReason::Expired => WireEndReason::SessionExpired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_end_reason_has_a_wire_kind() {
        assert!(matches!(
            wire_end_reason(SessionEndReason::ConnectionLost),
            WireEndReason::ConnectionLost
        ));
        assert!(matches!(
            wire_end_reason(SessionEndReason::UserLogout),
            WireEndReason::UserLogout
        ));
        assert!(matches!(
            wire_end_reason(SessionEndReason::Expired),
            WireEndReason::SessionExpired
        ));
    }
}
