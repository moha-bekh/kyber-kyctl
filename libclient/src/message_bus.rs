#![allow(dead_code)]

use log::warn;

use crate::{Error, Message, Result};

pub(crate) struct MessageWrapper {
    msg: Message,
}

impl MessageWrapper {
    pub(crate) fn msg(&self) -> &Message {
        &self.msg
    }
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

    pub(crate) async fn send_async(&self, msg: Message) -> Result<()> {
        self.send_wrapper_async(MessageWrapper {
            msg,
            completion_cb: None,
        })
        .await
        Ok(())
    }
}

pub(crate) fn create() -> (MessageSender, MessageReceiver) {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    (MessageSender { sender }, MessageReceiver { receiver })
}
