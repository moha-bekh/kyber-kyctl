// Project Kyber: host_events.rs
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

use serde::{Deserialize, Serialize};

// Control-plane WebSocket authentication frame, sent by the client as the first
// message after the connection is accepted.
#[derive(Debug, Deserialize, Serialize)]
pub struct WsAuth {
    pub token: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Display {
    pub id: u32,
    #[serde(default)]
    pub name: String,
    pub height: i32,
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum StreamerPipelineEventType {
    Stopped,
    Error,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StreamerPipelineEvent {
    pub type_: StreamerPipelineType,
    pub stage: StreamerPipelineStage,
    pub event: StreamerPipelineEventType,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum HostEvent {
    DisplayList { displays: Vec<Display> },
    StreamerPipelineEvent(StreamerPipelineEvent),
    BitrateChanged { bitrate: u32 },
}

#[derive(Debug, Deserialize, Serialize)]
pub enum HostEventFrame {
    Ping { id: u64 },
    Pong { id: u64 },
    Event { event: HostEvent },
}
