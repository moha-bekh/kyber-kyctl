// Project Kyber: mod.rs
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

pub(crate) mod dom;
mod host_events;
pub(crate) mod http;
mod input_service;
mod mainloop_engine;
pub(crate) mod message;

#[cfg(feature = "js-player")]
pub mod player;
pub(crate) mod reflect;
mod websocket;

use log::info;

use crate::{AuthCredentials, Result, VideoCodec};
pub(crate) use host_events::{WsClient, WsClientBuilder};
pub(crate) use input_service::{get_cursor_format, InputError, InputMsgSink, InputPipeline};
pub(crate) use mainloop_engine::MainLoopEngine;
pub(crate) use websocket::{WebSocket, WebSocketHandler};

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub credentials: AuthCredentials,
    /// Enable automatic reconnection on connection loss. Default: false.
    /// When enabled, the client will emit `Reconnecting`, `Reconnected`, and
    /// `ReconnectionFailed` events that the application must handle.
    pub auto_reconnection: bool,
}

#[derive(Clone, Debug)]
pub struct VideoPlayerConfig {
    pub canvas: String,
    pub software_decode: bool,
    pub notify_decoded: bool,
    pub video_buffer: u32, // ms
}

#[cfg(feature = "js-player")]
impl From<VideoPlayerConfig> for player::VideoPlayerConfig {
    fn from(config: VideoPlayerConfig) -> Self {
        Self {
            canvas: config.canvas,
            notify_decoded: config.notify_decoded,
            video_buffer: config.video_buffer,
        }
    }
}

impl VideoPlayerConfig {
    pub fn print(&self) {
        info!("VideoPlayerConfig");
        info!("\tcanvas: {}", self.canvas);
        info!("\tsoftware_decode: {}", self.software_decode);
        info!("\tnotify_decoded: {}", self.notify_decoded);
        info!("\tvideo_buffer: {}", self.video_buffer);
    }
}

#[derive(Clone, Debug)]
pub struct AudioPlayerConfig {
    pub audio_buffer: u32, // ms
}

#[cfg(feature = "js-player")]
impl From<AudioPlayerConfig> for player::AudioPlayerConfig {
    fn from(config: AudioPlayerConfig) -> Self {
        Self {
            audio_buffer: config.audio_buffer,
        }
    }
}

#[cfg(feature = "js-player")]
impl From<VideoCodec> for player::VideoCodec {
    fn from(codec: VideoCodec) -> Self {
        match codec {
            VideoCodec::H264 => player::VideoCodec::H264,
            VideoCodec::HEVC => player::VideoCodec::HEVC,
            VideoCodec::AV1 => player::VideoCodec::AV1,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) credentials: AuthCredentials,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        Ok(Self {
            host: connect_config.host.clone(),
            port: connect_config.port,
            credentials: connect_config.credentials,
        })
    }
}
