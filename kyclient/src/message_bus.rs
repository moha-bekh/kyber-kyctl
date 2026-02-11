// Project Kyber: message_bus.rs
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

#![allow(dead_code)]

use log::warn;

use crate::{Error, Message, Result};

#[cfg(not(target_arch = "wasm32"))]
pub(crate) type MessageCompletionCb = Box<dyn FnOnce() + Send>;

#[cfg(target_arch = "wasm32")]
pub(crate) type MessageCompletionCb = Box<dyn FnOnce()>;

pub(crate) struct MessageWrapper {
    msg: Message,
    completion_cb: Option<MessageCompletionCb>,
}

impl MessageWrapper {
    pub(crate) fn msg(&self) -> &Message {
        &self.msg
    }

    pub(crate) fn complete(self) {
        if let Some(completion_cb) = self.completion_cb {
            completion_cb();
        }
    }
}

pub(crate) struct MessageBlockingCompletionReceiver {
    rx: std::sync::mpsc::Receiver<()>,
}

impl MessageBlockingCompletionReceiver {
    pub(crate) fn wait(self) {
        let ret = self.rx.recv();
        if ret.is_err() {
            warn!("MessageBlockingCompletionReceiver::wait() failed");
        }
    }
}

pub(crate) struct MessageBlockingCompletionSender {
    tx: std::sync::mpsc::Sender<()>,
}

impl MessageBlockingCompletionSender {
    pub(crate) fn notify(self) {
        let ret = self.tx.send(());
        if ret.is_err() {
            warn!("MessageBlockingCompletionSender::notify() failed");
        }
    }
}

pub(crate) fn create_message_blocking_completion() -> (
    MessageBlockingCompletionSender,
    MessageBlockingCompletionReceiver,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    (
        MessageBlockingCompletionSender { tx },
        MessageBlockingCompletionReceiver { rx },
    )
}

pub(crate) struct MessageAsyncCompletionReceiver {
    rx: tokio::sync::oneshot::Receiver<()>,
}

impl MessageAsyncCompletionReceiver {
    pub(crate) async fn wait(self) {
        let ret = self.rx.await;
        if ret.is_err() {
            warn!("MessageAsyncCompletionReceiver::wait() failed");
        }
    }
}

pub(crate) struct MessageAsyncCompletionSender {
    tx: tokio::sync::oneshot::Sender<()>,
}

impl MessageAsyncCompletionSender {
    pub(crate) fn notify(self) {
        let ret = self.tx.send(());
        if ret.is_err() {
            warn!("MessageAsyncCompletionSender::notify() failed");
        }
    }
}

pub(crate) fn create_message_async_completion(
) -> (MessageAsyncCompletionSender, MessageAsyncCompletionReceiver) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (
        MessageAsyncCompletionSender { tx },
        MessageAsyncCompletionReceiver { rx },
    )
}

pub struct MessageReceiver {
    receiver: tokio::sync::mpsc::Receiver<MessageWrapper>,
}

impl MessageReceiver {
    pub(crate) async fn recv(&mut self) -> Option<MessageWrapper> {
        self.receiver.recv().await
    }
}

#[derive(Clone)]
pub struct MessageSender {
    sender: tokio::sync::mpsc::Sender<MessageWrapper>,
}

impl MessageSender {
    fn send_wrapper(&self, wrapper: MessageWrapper) -> Result<()> {
        use tokio::sync::mpsc::error::TrySendError;

        match self.sender.try_send(wrapper) {
            Ok(_) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(Error::ChannelSend),
            Err(TrySendError::Full(_)) => {
                warn!("MessageBus: Channel full: message dropped");

                Ok(())
            }
        }
    }

    async fn send_wrapper_async(&self, wrapper: MessageWrapper) -> Result<()> {
        self.sender.send(wrapper).await?;
        Ok(())
    }

    pub(crate) fn send(&self, msg: Message) -> Result<()> {
        self.send_wrapper(MessageWrapper {
            msg,
            completion_cb: None,
        })
    }

    pub(crate) fn send_and_wait_completion(&self, msg: Message) -> Result<()> {
        let (tx, rx) = create_message_blocking_completion();

        let completion_cb = Box::new(move || {
            tx.notify();
        });

        self.send_wrapper(MessageWrapper {
            msg,
            completion_cb: Some(completion_cb),
        })?;

        rx.wait();

        Ok(())
    }

    pub(crate) async fn send_async(&self, msg: Message) -> Result<()> {
        self.send_wrapper_async(MessageWrapper {
            msg,
            completion_cb: None,
        })
        .await
    }

    pub(crate) async fn send_async_and_wait_completion(&self, msg: Message) -> Result<()> {
        let (tx, rx) = create_message_async_completion();

        let completion_cb = Box::new(move || {
            tx.notify();
        });

        self.send_wrapper(MessageWrapper {
            msg,
            completion_cb: Some(completion_cb),
        })?;

        rx.wait().await;

        Ok(())
    }
}

pub(crate) fn create() -> (MessageSender, MessageReceiver) {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    (MessageSender { sender }, MessageReceiver { receiver })
}
