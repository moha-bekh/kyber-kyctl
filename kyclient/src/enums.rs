// Project Kyber: enums.rs
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

use crate::host_events;
use crate::metrics::Metric;
use crate::{
    StreamerPipelineEventType, StreamerPipelineStage, StreamerPipelineType, StreamingSessionConfig,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[repr(C)]
pub struct Display {
    pub id: u32,
    pub height: i32,
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug)]
pub enum TlsRootCert {
    Path(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug)]
pub enum ControlPlaneCloseReason {
    Eof,
    Closed {
        was_clean: bool,
        code: Option<u16>,
        reason: Option<String>,
    },
    Timeout,
}

#[derive(Clone, Debug)]
pub enum DataPlaneCloseReason {
    Closed,
    Error,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) enum Message {
    Connect,
    Disconnect {
        stop_session: bool,
    },

    Reconnect {
        attempt: u32,
    },

    ControlPlaneClosed {
        reason: ControlPlaneCloseReason,
    },

    DataPlaneClosed {
        reason: DataPlaneCloseReason,
    },

    StartStreamingSession {
        config: StreamingSessionConfig,
    },
    StopStreamingSession,

    AudioSetVolume {
        volume: f32,
    },
    AudioSetMute {
        mute: bool,
    },
    AudioVolumeChanged {
        volume: f32,
    },
    AudioMuteChanged {
        mute: bool,
    },

    SetBitrate {
        bitrate: u32,
    },
    BitrateChanged {
        bitrate: u32,
    },

    PlayerStarted,
    PlayerStopped,

    InputServiceConnected,
    InputServiceDisconnected,

    DisplayListUpdated {
        displays: Vec<Display>,
    },

    StreamerPipelineEvent(host_events::StreamerPipelineEvent),

    VideoDecodeError,
    AudioDecodeError,

    #[cfg(feature = "web")]
    ImageDecoded {
        pts: u64,
    },

    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
        frame_coded_width: i32,
        frame_coded_height: i32,
    },

    Metric(Metric),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    Requested,
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectFailedReason {
    NetworkError,
    Timeout,
    Unauthorized,
    ServerError,
    TlsVerification,
    TlsConfiguration,
    WebSocket,
    Protocol,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    ControlPlaneConnected {
        displays: Vec<Display>,
    },
    ControlPlaneConnectFailed {
        reason: ConnectFailedReason,
    },
    AuthenticationRequired {
        issuer: String,
        client_id: String,
    },
    DataPlaneConnectFailed,
    DataPlaneClosed,
    Reconnecting {
        attempt: u32,
        max_attempts: u32,
    },
    Reconnected {
        should_restart_streaming: bool,
    },
    ReconnectionFailed,
    Stopped {
        reason: StopReason,
    },
    DisplayListUpdated {
        displays: Vec<Display>,
    },
    InputServiceConnected,
    InputServiceDisconnected,
    StreamingStarted,
    StreamingStartFailed,
    StreamingStopped,
    #[cfg(feature = "web")]
    ImageDecoded {
        pts: u64,
    },
    AudioVolumeChanged {
        volume: f32,
    },
    AudioMuteChanged {
        mute: bool,
    },
    BitrateChanged {
        bitrate: u32,
    },
    #[cfg(feature = "web")]
    CanvasResized {
        width: i32,
        height: i32,
        frame_coded_width: i32,
        frame_coded_height: i32,
    },
    StreamerPipelineEvent {
        type_: StreamerPipelineType,
        stage: StreamerPipelineStage,
        event: StreamerPipelineEventType,
    },
}
