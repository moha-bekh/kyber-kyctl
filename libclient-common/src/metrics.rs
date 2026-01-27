// Project Kyber: metrics.rs
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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum Metric {
    Video(VideoMetric),
    NetworkLocal(NetworkStats),
    NetworkPing(NetworkPing),
    NetworkRemote(NetworkStats),
}

#[derive(Debug, Clone)]
pub struct VideoMetric {
    pub source_id: u32,
    pub pts: u64,
    pub ts: i64,
    pub data: VideoMetricData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoMetricData {
    FrameAcquired,
    FrameEncoding,
    FrameEncoded,
    FrameSent,
    FrameReceived,
    FrameDecoding,
    FrameDecoded,
    FramePrepared,
    FrameDisplayed,
    FrameSkipped,
}

// like kymux::ConnectionStats, but a struct local and serializable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStats {
    pub rtt_micros: Option<u64>,
    pub packets_lost: Option<u64>,
    pub dropped_packets: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkPing {
    pub offset_micros: i64,
    pub delay_micros: i64,
}
