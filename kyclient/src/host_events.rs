// Project Kyber: host_events.rs
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

use std::time::Duration;

use async_trait::async_trait;
use log::warn;

use crate::platform;
use crate::{
    ConnectionParams, ControlPlaneCapabilities, Display, Message, MessageSender, PeerInfo, Result,
};

pub(crate) use kyctl_common::host_events::{HostEvent, HostEventFrame};
pub use kyctl_common::host_events::{
    StreamerPipelineEvent, StreamerPipelineEventType, StreamerPipelineStage, StreamerPipelineType,
};

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
pub(crate) trait WsClientBuilder {
    type Type;

    async fn new(
        conn_params: &ConnectionParams,
        peer_info: &PeerInfo,
        capabilities: &ControlPlaneCapabilities,
        event_sink: HostEventSink,
        auth_token: &str,
    ) -> Result<Self::Type>;
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
            HostEvent::DisplayList { displays } => Message::DisplayListUpdated {
                displays: displays.into_iter().map(Display::from).collect(),
            },
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
    _client: platform::WsClient,
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

        let _client = platform::WsClientBuilder::new(
            conn_params,
            peer_info,
            capabilities,
            event_sink,
            auth_token,
        )
        .await?;

        Ok(Self { _client })
    }
}

pub(crate) fn timeout_duration(capabilities: &ControlPlaneCapabilities) -> Duration {
    Duration::from_secs(capabilities.keepintvl * capabilities.keepcnt)
}
