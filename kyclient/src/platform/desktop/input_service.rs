// Project Kyber: input_service.rs
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

use log::{info, warn};

use kynput_rs::{
    Consumer, InputTarget, InputType, InputTypeMetadata, NetworkStream, NetworkStreamMsg,
    NetworkStreamObserver, Pipeline,
};

use crate::error::Result;
use crate::{CursorFormat, Message, MessageSender};

pub(crate) struct InputMsgSink {
    message_sender: MessageSender,
}

impl InputMsgSink {
    pub(crate) fn new(message_sender: MessageSender) -> Self {
        Self { message_sender }
    }
}

impl NetworkStreamObserver for InputMsgSink {
    fn on_message(&self, msg: NetworkStreamMsg) {
        let service_msg = match msg {
            NetworkStreamMsg::Serving => None,
            NetworkStreamMsg::ClientConnected => Some(Message::InputServiceConnected),
            NetworkStreamMsg::ClientDisconnected => Some(Message::InputServiceDisconnected),
        };

        if let Some(service_msg) = service_msg {
            let ret = self.message_sender.send(service_msg);
            if ret.is_err() {
                warn!("Failed to send NetworkStreamMsg to MessageSender");
            }
        }
    }
}

pub(crate) struct InputClient {
    _message_sender: MessageSender,
    stream: NetworkStream,
}

impl InputClient {
    pub(crate) fn new<F>(
        message_sender: MessageSender,
        rx_consumer: Consumer,
        stream_factory: F,
    ) -> Result<Self>
    where
        F: FnOnce(InputTarget, Box<dyn NetworkStreamObserver>) -> NetworkStream,
    {
        let msg_sink = Box::new(InputMsgSink::new(message_sender.clone()));
        let stream = stream_factory(InputTarget::Client, msg_sink);
        stream.plug_consumer(&rx_consumer)?;
        info!("run_client");
        stream.run()?;

        Ok(Self {
            _message_sender: message_sender,
            stream,
        })
    }

    pub fn get_tx_consumer(&self) -> Consumer {
        self.stream.clone_consumer()
    }

    pub fn stop(&mut self) -> Result<()> {
        self.stream.stop()?;

        Ok(())
    }
}

pub(crate) type InputError = kynput_rs::Error;
pub(crate) type InputPipeline = kynput_rs::Pipeline;

pub(crate) fn get_cursor_format(pipeline: &Pipeline) -> Option<CursorFormat> {
    let Some(InputTypeMetadata::Cursor { format }) =
        pipeline.query_type_metadata(InputType::Cursor)
    else {
        return None;
    };

    match format {
        kynput_rs::CursorFormat::Raw => Some(CursorFormat::Raw),
        kynput_rs::CursorFormat::Png => Some(CursorFormat::Png),
    }
}
