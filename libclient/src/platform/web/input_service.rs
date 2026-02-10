// Project Kyber: input_service.rs
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

use std::sync::Arc;

use log::warn;

use kynput::{InputNetworkStreamMsg, InputNetworkStreamObserver, InputType, InputTypeMetadata};

use crate::{CursorFormat, Message, MessageSender};

pub(crate) struct InputMsgSink {
    message_sender: MessageSender,
}

impl InputMsgSink {
    pub(crate) fn new(message_sender: MessageSender) -> Self {
        Self { message_sender }
    }
}

impl InputNetworkStreamObserver for InputMsgSink {
    fn on_message(&self, msg: InputNetworkStreamMsg) {
        let service_msg = match msg {
            InputNetworkStreamMsg::Serving => None,
            InputNetworkStreamMsg::ClientConnected => Some(Message::InputServiceConnected),
            InputNetworkStreamMsg::ClientDisconnected => Some(Message::InputServiceDisconnected),
        };

        if let Some(service_msg) = service_msg {
            let ret = self.message_sender.send(service_msg);
            if let Err(err) = ret {
                warn!("Failed to send message {msg:?}: {err:?}");
            }
        }
    }
}

pub(crate) type InputError = kynput::Error;
pub(crate) type InputPipeline = Arc<kynput::InputPipeline>;

pub(crate) fn get_cursor_format(pipeline: &InputPipeline) -> Option<CursorFormat> {
    let Some(InputTypeMetadata::Cursor { format }) =
        pipeline.query_type_metadata(InputType::Cursor)
    else {
        return None;
    };

    match format {
        kynput::CursorFormat::None => None,
        kynput::CursorFormat::Raw => Some(CursorFormat::Raw),
        kynput::CursorFormat::Png => Some(CursorFormat::Png),
    }
}
