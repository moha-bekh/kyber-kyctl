// Project Kyber: kymux.rs
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

use kymux::kyproto;
use serde::{Deserialize, Serialize};

use crate::enums::{CursorFormat, VideoCodec};

pub use kyproto::{AudioProtocol, VideoProtocol};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct KymuxConfig {
    pub video_protocol: Option<kyproto::VideoProtocol>,
    pub audio_protocol: Option<kyproto::AudioProtocol>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartKymux {
    pub public_streamer_port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequestVideo {
    pub display_id: u32,
    pub codec: VideoCodec,
    pub yuv444: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intra_refresh: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequestInput {
    pub cursor_format: CursorFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gamepad_force_xbox360: Option<bool>,
    #[serde(default)]
    pub clipboard: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StartKymuxCertificateHash {
    pub algorithm: String,
    pub hash: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartKymuxResponse {
    pub port: u16,
    pub certificate_hash: Option<StartKymuxCertificateHash>,
    pub token: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionVideoResponse {
    pub display_id: u32,
    pub endpoint: u16,
    pub metrics_endpoint: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionAudioResponse {
    pub endpoint: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionInputResponse {
    pub endpoint: u16,
    #[serde(default)]
    pub clipboard: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionClockSyncResponse {
    pub endpoint: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionMetricsResponse {
    pub endpoint: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionResponse {
    pub session_id: Option<u32>,
    pub video: Option<Vec<StartSessionVideoResponse>>,
    pub audio: Option<StartSessionAudioResponse>,
    pub input: Option<StartSessionInputResponse>,
    pub clock_sync: Option<StartSessionClockSyncResponse>,
    pub metrics: Option<StartSessionMetricsResponse>,
}
