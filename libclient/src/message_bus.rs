#![allow(dead_code)]

use log::warn;

use crate::{Error, Message, Result};

pub struct MessageReceiver {
    receiver: tokio::sync::mpsc::Receiver<Message>,
}

impl MessageReceiver {
    pub(crate) async fn recv(&mut self) -> Option<Message> {
        self.receiver.recv().await
    }
}

#[derive(Clone)]
pub struct MessageSender {
    sender: tokio::sync::mpsc::Sender<Message>,
}

impl MessageSender {
    pub(crate) fn send(&self, msg: Message) -> Result<()> {
        use tokio::sync::mpsc::error::TrySendError;

        match self.sender.try_send(msg) {
            Ok(_) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(Error::ChannelSend),
            Err(TrySendError::Full(_)) => {
                warn!("MessageBus: Channel full: message dropped");

                Ok(())
            }
        }
    }

    pub(crate) async fn send_async(&self, msg: Message) -> Result<()> {
        self.sender.send(msg).await?;
        Ok(())
    }
}

pub(crate) fn create() -> (MessageSender, MessageReceiver) {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    (MessageSender { sender }, MessageReceiver { receiver })
}
