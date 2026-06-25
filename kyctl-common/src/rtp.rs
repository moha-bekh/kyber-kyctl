// Project Kyber: rtp.rs
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

use crate::enums::CursorFormat;

#[derive(Debug, Deserialize, Serialize)]
pub struct HelloResponse {
    pub ip: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GenerateSdpRequest {
    pub yuv444: bool,
    pub display_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intra_refresh: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GenerateSdpResponse {
    pub params: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequestVideo {
    pub port: u16,
    pub display_id: u32,
    pub yuv444: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intra_refresh: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequestAudio {
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequestInput {
    pub cursor_format: CursorFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gamepad_force_xbox360: Option<bool>,
    #[serde(default)]
    pub clipboard: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequest {
    pub ip: String,
    pub bitrate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<StartRequestVideo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<StartRequestAudio>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<StartRequestInput>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartResponse {
    pub session_id: Option<u32>,
}
