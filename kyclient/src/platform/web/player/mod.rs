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

mod audio;
pub mod packet;
mod video;

pub(crate) use audio::player::{
    AudioPlayer, AudioPlayerEvent, AudioPlayerHandle, AudioPlayerListener,
};
pub(crate) use video::player::{
    VideoPlayer, VideoPlayerEvent, VideoPlayerHandle, VideoPlayerListener, VideoPlayerMetric,
};
pub(crate) use video::{get_video_decoder_config, supported_video_codecs, VideoDecoderConfig};

use packet::Packet;

use crate::Result;

pub(crate) trait PacketSink {
    fn send_packet(&mut self, packet: Packet) -> Result<()>;
}
