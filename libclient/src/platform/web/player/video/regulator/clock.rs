// Project Kyber: clock.rs
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

/**
 * Clock estimator
 *
 * This clock computes an exponentially weighted moving average (EWMA) of the
 * offset between the stream time (PTS) and the system time of received frames.
 *
 * The estimation is then used to compute the target system time a frame with a
 * certain PTS should be displayed.
 */
pub(crate) struct Clock {
    pub range: usize,
    pub offset: u64,
}

impl Clock {
    const RANGE: usize = 32;

    pub fn new() -> Self {
        Self {
            range: 0,
            offset: 0,
        }
    }

    pub fn update(&mut self, system: u64, stream: u64) {
        if self.range < Self::RANGE {
            self.range += 1
        }

        let offset = system - stream;
        self.offset = ((self.range as u64 - 1) * self.offset + offset) / self.range as u64;
    }

    pub fn to_system_time(&self, stream: u64) -> u64 {
        assert!(self.range > 0);
        stream + self.offset
    }
}
