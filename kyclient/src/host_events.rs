// Project Kyber: host_events.rs
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

use std::time::Duration;

use async_trait::async_trait;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::platform;
use crate::{
    ConnectionParams, ControlPlaneCapabilities, Display, Message, MessageSender, PeerInfo, Result,
};

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub(crate) trait WsClient {
    async fn stop(self) -> Result<()>;
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub(crate) trait WsClientBuilder {
    type Type: WsClient;

    async fn new(
        conn_params: &ConnectionParams,
        peer_info: &PeerInfo,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
        auth_token: &str,
    ) -> Result<Self::Type>;
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

#[derive(Debug, Deserialize, Serialize)]
pub(crate) enum HostEvent {
    DisplayList { displays: Vec<Display> },
    StreamerPipelineEvent(StreamerPipelineEvent),
    BitrateChanged { bitrate: u32 },
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) enum HostEventFrame {
    Event { event: HostEvent },
    Ping { id: u64 },
    Pong { id: u64 },
}

pub(crate) struct HostEventSink {
    msg_sender: MessageSender,
}

impl HostEventSink {
    fn new(msg_sender: MessageSender) -> Self {
        Self { msg_sender }
    }

    fn convert_event(event: HostEvent) -> Message {
        match event {
            HostEvent::DisplayList { displays } => Message::DisplayListUpdated { displays },
            HostEvent::StreamerPipelineEvent(event) => Message::StreamerPipelineEvent(event),
            HostEvent::BitrateChanged { bitrate } => Message::BitrateChanged { bitrate },
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn push_msg(&self, msg: Message) {
        let ret = self.msg_sender.send_async(msg).await;
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }

    #[allow(dead_code)]
    pub(crate) fn blocking_push_msg(&self, msg: Message) {
        let ret = self.msg_sender.send(msg);
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }

    #[allow(dead_code)]
    pub(crate) async fn push(&self, event: HostEvent) {
        let ret = self.msg_sender.send_async(Self::convert_event(event)).await;
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }

    #[allow(dead_code)]
    pub(crate) fn blocking_push(&self, event: HostEvent) {
        let ret = self.msg_sender.send(Self::convert_event(event));
        if let Err(err) = ret {
            warn!("Fail to send HostEvent: {err:?}");
        }
    }
}

pub(crate) struct HostEventReceiver {
    client: platform::WsClient,
}

impl HostEventReceiver {
    pub(crate) async fn new(
        conn_params: &ConnectionParams,
        peer_info: &PeerInfo,
        capabilities: &ControlPlaneCapabilities,
        msg_sender: MessageSender,
        auth_token: &str,
    ) -> Result<Self> {
        let event_sink = HostEventSink::new(msg_sender);

        let client = platform::WsClientBuilder::new(
            conn_params,
            peer_info,
            capabilities,
            event_sink,
            auth_token,
        )
        .await?;

        Ok(Self { client })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.client.stop().await
    }
}

pub(crate) fn timeout_duration(capabilities: &ControlPlaneCapabilities) -> Duration {
    Duration::from_secs(capabilities.keepintvl * capabilities.keepcnt)
}
