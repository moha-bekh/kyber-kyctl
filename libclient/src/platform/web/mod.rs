pub(crate) mod dom;
mod host_events;
pub(crate) mod http;
mod input_service;

use crate::Result;
pub(crate) use host_events::{WsClient, WsClientBuilder};
pub(crate) use input_service::{get_cursor_format, InputError, InputMsgSink, InputPipeline};

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub canvas: String,
    pub software_decode: bool,
}

#[derive(Clone)]
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        Ok(Self {
            host: connect_config.host.clone(),
            port: connect_config.port,
        })
    }
}
