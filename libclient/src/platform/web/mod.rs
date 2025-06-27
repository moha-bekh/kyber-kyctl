pub(crate) mod dom;
mod host_events;
pub(crate) mod http;
mod input_service;
mod mainloop_engine;

#[cfg(feature = "js-player")]
pub(crate) mod player;
pub(crate) mod reflect;
mod websocket;

use log::info;

use crate::{AuthCredentials, Result};
pub(crate) use host_events::{WsClient, WsClientBuilder};
pub(crate) use input_service::{get_cursor_format, InputError, InputMsgSink, InputPipeline};
pub(crate) use mainloop_engine::MainLoopEngine;
pub(crate) use websocket::{WebSocket, WebSocketHandler};

#[derive(Clone, Debug)]
pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub credentials: AuthCredentials,
}

#[derive(Clone, Debug)]
pub struct VideoPlayerConfig {
    pub canvas: String,
    pub software_decode: bool,
    pub notify_decoded: bool,
    pub video_buffer: u32, // ms
}

impl VideoPlayerConfig {
    pub fn print(&self) {
        info!("VideoPlayerConfig");
        info!("\tcanvas: {}", self.canvas);
        info!("\tsoftware_decode: {}", self.software_decode);
        info!("\tnotify_decoded: {}", self.notify_decoded);
        info!("\tvideo_buffer: {}", self.video_buffer);
    }
}

#[derive(Clone, Debug)]
pub struct AudioPlayerConfig {
    pub audio_buffer: u32, // ms
}

#[derive(Clone)]
pub(crate) struct ConnectionParams {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) credentials: AuthCredentials,
}

impl ConnectionParams {
    pub(crate) fn new(connect_config: ConnectConfig) -> Result<Self> {
        Ok(Self {
            host: connect_config.host.clone(),
            port: connect_config.port,
            credentials: connect_config.credentials,
        })
    }
}
