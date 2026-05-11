// Project Kyber: error.rs
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

use std::path::PathBuf;

use crate::platform;

#[cfg(feature = "kymux")]
use kymux::{kynet, kyproto};

#[cfg(feature = "desktop")]
use platform::player::PlayerError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("no peer address")]
    NoPeerAddress,
    #[error("invalid address {addr:?}")]
    InvalidAddress { addr: String },
    #[error("failed to resolve address {addr:?}")]
    FailToResolveAddress { addr: String },
    #[error("string is not valid unicode")]
    InvalidUnicodeStr,
    #[error("failed to send message to channel")]
    ChannelSend,
    #[error("command or request was rejected")]
    Rejected,
    #[error("thread panicked")]
    ThreadPanicked,
    #[error("no input service")]
    NoInputService,
    #[error("no player")]
    NoPlayer,
    #[error("no audio player")]
    NoAudioPlayer,
    #[error("input error: {0}")]
    Input(#[from] platform::InputError),
    #[cfg(feature = "kymux")]
    #[error("kymux not started")]
    KymuxNotStarted,
    #[cfg(feature = "kymux")]
    #[error("kymux ready failed: {reason}")]
    KymuxReadyFailed { reason: String },
    #[cfg(feature = "kymux")]
    #[error("kymux send failed: {reason}")]
    KymuxSendFailed { reason: String },
    #[cfg(feature = "kymux")]
    #[error("kymux invalid token")]
    KymuxInvalidToken,
    #[cfg(feature = "kymux")]
    #[error("kynet connection error: {0}")]
    KynetConnectionError(#[from] kynet::ConnectionError),
    #[cfg(feature = "kymux")]
    #[error("kynet decode hex error: {0}")]
    DecodeHexError(#[from] kynet::DecodeHexError),
    #[cfg(feature = "kymux")]
    #[error("kyproto protocol error: {0}")]
    KyProtoProtocolError(#[from] kyproto::ProtocolError),
    #[cfg(feature = "desktop")]
    #[error("failed to join async task: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),
    #[error("file not found: {0:?}")]
    FileNotFound(PathBuf),
    #[error("invalid tls fingerprint")]
    TlsInvalidFingerprint,
    #[error("tls verification failed")]
    TlsVerificationFailed,
    #[error("invalid state")]
    InvalidState,

    #[cfg(feature = "web")]
    #[error("js error: {0:?}")]
    JsError(wasm_bindgen::JsValue),
    #[cfg(feature = "web")]
    #[error("js dom invalid")]
    JsDomInvalid,
    #[cfg(feature = "web")]
    #[error("SharedArrayBuffer unvailable")]
    SharedArrayBufferUndefined,

    // Player errors
    #[cfg(feature = "desktop")]
    #[error("player error: {0}")]
    Player(#[from] PlayerError),
    // Host service errors
    #[error("http client error: {0}")]
    HttpClient(#[from] reqwest::Error),
    #[error("http request timed out")]
    HttpQueryTimeout,
    #[error("http error {status}")]
    HttpQueryFailed { status: u16 },
    #[error("WebSocket connection failed")]
    WsConnectFailed,
    #[error("data-plane connection failed")]
    DataPlaneConnectFailed,
    #[error("deserialization failed")]
    DeserializationFailed,
}

impl<T> From<tokio::sync::mpsc::error::SendError<T>> for Error {
    fn from(_err: tokio::sync::mpsc::error::SendError<T>) -> Error {
        Error::ChannelSend
    }
}

impl<T> From<std::sync::PoisonError<T>> for Error {
    fn from(_: std::sync::PoisonError<T>) -> Error {
        Error::ThreadPanicked
    }
}

#[cfg(feature = "web")]
impl From<wasm_bindgen::JsValue> for Error {
    fn from(err: wasm_bindgen::JsValue) -> Error {
        Error::JsError(err)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
