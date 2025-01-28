use log::{info, warn};

use libkynput_rs::{
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

    pub async fn stop(&mut self) -> Result<()> {
        self.stream.stop()?;

        Ok(())
    }
}

pub(crate) type InputError = libkynput_rs::Error;
pub(crate) type InputPipeline = libkynput_rs::Pipeline;

pub(crate) fn get_cursor_format(pipeline: &Pipeline) -> Option<CursorFormat> {
    let Some(InputTypeMetadata::Cursor { format }) =
        pipeline.query_type_metadata(InputType::Cursor)
    else {
        return None;
    };

    match format {
        libkynput_rs::CursorFormat::Raw => Some(CursorFormat::Raw),
        libkynput_rs::CursorFormat::Png => Some(CursorFormat::Png),
    }
}
