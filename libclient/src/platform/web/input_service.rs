use std::sync::Arc;

use log::warn;

use libkynput::{InputNetworkStreamMsg, InputNetworkStreamObserver, InputType, InputTypeMetadata};

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

pub(crate) type InputError = libkynput::Error;
pub(crate) type InputPipeline = Arc<libkynput::InputPipeline>;

pub(crate) fn get_cursor_format(pipeline: &InputPipeline) -> Option<CursorFormat> {
    let Some(InputTypeMetadata::Cursor { format }) =
        pipeline.query_type_metadata(InputType::Cursor)
    else {
        return None;
    };

    match format {
        libkynput::CursorFormat::None => None,
        libkynput::CursorFormat::Raw => Some(CursorFormat::Raw),
        libkynput::CursorFormat::Png => Some(CursorFormat::Png),
    }
}
