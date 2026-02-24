// Project Kyber: error.rs
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

use std::path::PathBuf;

use kymux::kyproto;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("IO Error: {source}")]
    IO {
        #[from]
        source: std::io::Error,
    },
    #[error("kymux error: {source}")]
    Kymux {
        #[from]
        source: kymux::Error,
    },
    #[error("kyproto protocol error: {source}")]
    KyProtoProtocol {
        #[from]
        source: kyproto::ProtocolError,
    },
    #[error("libkypc error: {source}")]
    Libypc {
        #[from]
        source: libkypc::Error,
    },
    #[error("Streamer already started")]
    StreamerStarted,
    #[error("Streamer already stopped")]
    StreamerStopped,
    #[error("Control plane not opened")]
    ControlPlaneNotOpened,
    #[error("Data plane already opened")]
    DataPlaneAlreadyOpened,
    #[error("Invalid session \"{uid}\"")]
    InvalidSession { uid: uuid::Uuid },
    #[error("Kymux accept() timeout")]
    KymuxAcceptTimeout,
    #[error("Kymux isn't listening")]
    KymuxNotListening,
    #[error("Got an unexpected response from AvServer")]
    AvServerUnexpectedResponse,
    #[error("Couldn't get a receiver from AvServer")]
    AvServerNoReceiver,
    #[error("Couldn't open file {path}")]
    FileNotFound { path: PathBuf },
    #[error("Invalid TLS file")]
    TlsInvalidFile,
    #[error("rustls error: {source}")]
    Rustls {
        #[from]
        source: rustls::Error,
    },
    #[error("rustls default provider failed to initialize")]
    RustlsDefaultProviderInit,
    #[error("rcgen error: {source}")]
    RcGen {
        #[from]
        source: rcgen::Error,
    },
    #[error("Operation on date overflow/underflow")]
    DateOperationOverflow,
    #[error("Actix Web error: {0}")]
    Actix(String),
    #[error("Thread panicked")]
    ThreadPanicked,
    #[error("Channel receive failed (channel closed)")]
    ChannelClosedRx,
    #[error("Channel send failed (channel closed)")]
    ChannelClosedTx,
    #[error("DisplayList empty")]
    DisplayListEmpty,
    #[error("No Websocket router")]
    WsNoRouter,
    #[error("Websocket component {component} not ready")]
    WsComponentNotReady { component: String },
    #[error("Websocket got invalid endpoint {endpoint_id:X} from process")]
    WsInvalidEndpoint { endpoint_id: u16 },
    #[error("Invalid authentication message")]
    WsInvalidAuthMessage,
    #[error("Authentication timed out")]
    WsAuthTimeout,
    #[error("Config parse failed: {source}")]
    ConfigParseFailed {
        #[from]
        source: crate::config::ParseError,
    },
    #[error("No statemachine proxy")]
    NotStateMachineProxy,
}

pub type Result<T> = std::result::Result<T, Error>;

// actix_web::Error is not Send, we want our error::Error to be Send
impl From<actix_web::Error> for Error {
    fn from(err: actix_web::Error) -> Self {
        Error::Actix(err.to_string())
    }
}
