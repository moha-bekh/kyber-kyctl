use log::info;

use libkynput_rs::{Consumer, InputTarget, NetworkStream, NetworkStreamMsg, NetworkStreamObserver};

use super::{Message, MessageProxy};
use crate::error::Result;

struct MsgSink {
    message_proxy: MessageProxy,
}

impl MsgSink {
    fn new(message_proxy: MessageProxy) -> Self {
        Self { message_proxy }
    }
}

impl NetworkStreamObserver for MsgSink {
    fn on_message(&self, msg: NetworkStreamMsg) {
        let service_msg = match msg {
            NetworkStreamMsg::Serving => None,
            NetworkStreamMsg::ClientConnected => Some(Message::InputServiceConnected),
            NetworkStreamMsg::ClientDisconnected => Some(Message::InputServiceDisconnected),
        };

        if let Some(service_msg) = service_msg {
            self.message_proxy.send(service_msg).unwrap();
        }
    }
}

pub struct Client {
    _message_proxy: MessageProxy,
    stream: NetworkStream,
}

impl Client {
    pub fn new<F>(
        message_proxy: MessageProxy,
        rx_consumer: Consumer,
        stream_factory: F,
    ) -> Result<Self>
    where
        F: FnOnce(InputTarget, Box<dyn NetworkStreamObserver>) -> NetworkStream,
    {
        let msg_sink = Box::new(MsgSink::new(message_proxy.clone()));
        let stream = stream_factory(InputTarget::Client, msg_sink);
        stream.plug_consumer(&rx_consumer)?;
        info!("run_client");
        stream.run()?;

        Ok(Self {
            _message_proxy: message_proxy,
            stream,
        })
    }

    pub fn get_tx_consumer(&self) -> Consumer {
        self.stream.clone_consumer()
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.stream.stop()?;

        Ok(())
    }
}
