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
    tcp_service: NetworkStream,
}

impl Client {
    pub fn new(
        consumer_ip: &str,
        message_proxy: MessageProxy,
        rx_consumer: Consumer,
    ) -> Result<Self> {
        let host = String::from(consumer_ip);

        let msg_sink = Box::new(MsgSink::new(message_proxy.clone()));
        let tcp_service = libkynput_rs::create_input_stream_tcp(InputTarget::Client, msg_sink);
        tcp_service.plug_consumer(&rx_consumer)?;
        info!("run_client");
        tcp_service.run_client(&host)?;

        Ok(Self {
            _message_proxy: message_proxy,
            tcp_service,
        })
    }

    pub fn get_tx_consumer(&self) -> Consumer {
        self.tcp_service.clone_consumer()
    }

    pub async fn stop(&mut self) -> Result<()> {
        self.tcp_service.stop()?;

        Ok(())
    }
}
