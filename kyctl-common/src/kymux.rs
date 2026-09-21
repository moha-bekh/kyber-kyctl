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

#[derive(Default, Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum VideoProtocol {
    #[default]
    Reliable,
    GopStream,
    Unreliable,
    UnreliableFec,
}

#[derive(Default, Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum AudioProtocol {
    #[default]
    Reliable,
    Unreliable,
    UnreliableFec,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct KyProtoConfig {
    #[serde(default)]
    pub video_protocol: VideoProtocol,
    #[serde(default)]
    pub audio_protocol: AudioProtocol,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum KymuxConfig {
    KyProto(KyProtoConfig),
    KyWebSocket,
}

impl KymuxConfig {
    pub fn kyproto_config(&self) -> &KyProtoConfig {
        match self {
            KymuxConfig::KyProto(config) => config,
            KymuxConfig::KyWebSocket => panic!("Not a kyproto transport"),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartKymux {
    pub public_streamer_port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartRequest {
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    pub bitrate: u32,

    pub kymux_config: KymuxConfig,
    pub video: Option<Vec<StartRequestVideo>>,
    pub audio: bool,
    pub inputs: Option<StartRequestInput>,
    pub metrics: bool,
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

// Endpoints are Some(endpoint_id) for kyproto, but None for kywebsockets
// (websockets have no endpoint id).

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionVideoResponse {
    pub display_id: u32,
    pub endpoint: Option<u16>,
    pub metrics_endpoint: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionAudioResponse {
    pub endpoint: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionInputResponse {
    pub endpoint: Option<u16>,
    #[serde(default)]
    pub clipboard: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionClockSyncResponse {
    pub endpoint: Option<u16>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StartSessionMetricsResponse {
    pub endpoint: Option<u16>,
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

impl From<VideoProtocol> for kyproto::VideoProtocol {
    fn from(protocol: VideoProtocol) -> Self {
        match protocol {
            VideoProtocol::Reliable => Self::Reliable,
            VideoProtocol::GopStream => Self::GopStream,
            VideoProtocol::Unreliable => Self::Unreliable,
            VideoProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

impl From<kyproto::VideoProtocol> for VideoProtocol {
    fn from(protocol: kyproto::VideoProtocol) -> Self {
        match protocol {
            kyproto::VideoProtocol::Reliable => Self::Reliable,
            kyproto::VideoProtocol::GopStream => Self::GopStream,
            kyproto::VideoProtocol::Unreliable => Self::Unreliable,
            kyproto::VideoProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

impl From<AudioProtocol> for kyproto::AudioProtocol {
    fn from(protocol: AudioProtocol) -> Self {
        match protocol {
            AudioProtocol::Reliable => Self::Reliable,
            AudioProtocol::Unreliable => Self::Unreliable,
            AudioProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

impl From<kyproto::AudioProtocol> for AudioProtocol {
    fn from(protocol: kyproto::AudioProtocol) -> Self {
        match protocol {
            kyproto::AudioProtocol::Reliable => Self::Reliable,
            kyproto::AudioProtocol::Unreliable => Self::Unreliable,
            kyproto::AudioProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}
