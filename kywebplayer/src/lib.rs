// Project Kyber: mod.rs
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

mod audio;
pub mod packet;
mod reflect;
mod video;

pub use audio::player::{
    AudioPlayer, AudioPlayerCreationError, AudioPlayerEvent, AudioPlayerHandle, AudioPlayerListener,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioResumeState {
    /// No AudioContext exists yet; the caller should retry later.
    Absent,
    /// Context is running, audio is playing.
    Running,
    /// Context is not running yet. Either a resume is in progress (a gesture was
    /// active and the next poll will report `Running`) or the context is waiting
    /// for a user gesture. Either way the caller should keep polling.
    Suspended,
}
pub use video::player::{
    VideoPlayer, VideoPlayerEvent, VideoPlayerHandle, VideoPlayerListener, VideoPlayerMetric,
};
pub use video::{supported_video_codecs, VideoCodec, VideoDecoderConfig};

use packet::Packet;

use wasm_bindgen::prelude::*;

#[derive(Clone, Debug)]
pub struct VideoPlayerConfig {
    pub canvas: String,
    pub app_crate_name: String,
    pub force_software_decoding: bool,
    pub notify_decoded: bool,
    pub video_buffer: u32, // ms
    pub yuv444: bool,      // HACK: should be parsed from the config packet
}

#[derive(Clone, Debug)]
pub struct AudioPlayerConfig {
    pub audio_buffer: u32, // ms
}

pub trait PacketSink {
    fn send_packet(&mut self, packet: Packet) -> Result<(), JsValue>;
}
