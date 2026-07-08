// Project Kyber: message_bus.rs
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

#![allow(dead_code)]

use log::warn;

use crate::{Error, Message, Result};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type MessageCompletionCb = Box<dyn FnOnce() + Send>;

#[cfg(target_arch = "wasm32")]
pub(crate) type MessageCompletionCb = Box<dyn FnOnce()>;

pub(crate) struct MessageWrapper {
    pub msg: Message,
    pub completion_cb: Option<MessageCompletionCb>,
}

impl std::fmt::Debug for MessageWrapper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message").field("msg", &self.msg).finish()
    }
}

pub struct MessageReceiver {
    receiver: tokio::sync::mpsc::UnboundedReceiver<MessageWrapper>,
}

impl MessageReceiver {
    pub(crate) async fn recv(&mut self) -> Option<MessageWrapper> {
        self.receiver.recv().await
    }
}

#[derive(Clone)]
pub struct MessageSender {
    sender: tokio::sync::mpsc::UnboundedSender<MessageWrapper>,
}

impl MessageSender {
    fn send_wrapper(&self, wrapper: MessageWrapper) -> Result<()> {
        self.sender.send(wrapper).map_err(|_| Error::ChannelSend)?;
        Ok(())
    }

    pub(crate) fn send(&self, msg: Message) -> Result<()> {
        self.send_wrapper(MessageWrapper {
            msg,
            completion_cb: None,
        })
    }

    pub(crate) fn send_and_wait_completion(&self, msg: Message) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::channel();

        let completion_cb = Box::new(move || {
            if tx.send(()).is_err() {
                warn!("Notify failed");
            }
        });

        self.send_wrapper(MessageWrapper {
            msg,
            completion_cb: Some(completion_cb),
        })?;

        if rx.recv().is_err() {
            warn!("Wait failed");
        }

        Ok(())
    }

    pub(crate) async fn send_async_and_wait_completion(&self, msg: Message) -> Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        let completion_cb = Box::new(move || {
            if tx.send(()).is_err() {
                warn!("Notify failed");
            }
        });

        self.send_wrapper(MessageWrapper {
            msg,
            completion_cb: Some(completion_cb),
        })?;

        if rx.await.is_err() {
            warn!("Wait failed");
        }

        Ok(())
    }
}

pub(crate) fn create() -> (MessageSender, MessageReceiver) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    (MessageSender { sender }, MessageReceiver { receiver })
}
