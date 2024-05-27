mod audio;
mod host_service;
mod input;
mod video;

use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};

use libkynput::bufferproxy::BufferProxy;
use libkynput::{InputNetworkStream, InputPipeline};

use audio::AudioWsSocketHandler;
use input::InputWebSocketHandler;
use video::VideoWsSocketHandler;

use crate::platform::get_cursor_format;
use crate::platform::player::{get_video_decoder_config, AudioPlayer, VideoPlayer};
use crate::platform::InputMsgSink;

use crate::http::HttpClient;
use crate::{
    BackendDriver, ConnectionParams, MessageProxy, StreamingSessionConfig, VideoSessionConfig,
};
use crate::{Error, Result};

struct VideoService {
    ws: Rc<VideoWsSocketHandler>,
    player: Rc<VideoPlayer>,
}

impl VideoService {
    async fn stop(self) {
        info!("Stop video service");

        let ret = self.ws.stop();
        if let Err(err) = ret {
            warn!("Video ws stop failed: {err:?}");
        }

        let ret = self.player.stop().await;
        if let Err(err) = ret {
            warn!("Video player stop failed: {err:?}");
        }
    }
}

struct AudioService {
    ws: Rc<AudioWsSocketHandler>,
    player: Rc<AudioPlayer>,
}

impl AudioService {
    async fn stop(self) {
        info!("Stop audio service");

        let ret = self.ws.stop();
        if let Err(err) = ret {
            warn!("Audio ws stop failed: {err:?}");
        }

        let ret = self.player.stop().await;
        if let Err(err) = ret {
            warn!("Audio player stop failed: {err:?}");
        }
    }
}

struct InputService {
    websocket: Rc<InputWebSocketHandler>,
    proxy: Arc<BufferProxy>,
    pipeline: Arc<libkynput::InputPipeline>,
}

impl InputService {
    fn stop(self) {
        info!("Stop input service");
        let ret = self.websocket.stop();
        if let Err(err) = ret {
            warn!("Input ws stop failed: {err:?}");
        }

        let ret = self.pipeline.stop();
        if let Err(err) = ret {
            warn!("InputPipeline stop failed: {err:?}");
        }
    }
}

pub(crate) struct WsBackend {
    video_service: Option<VideoService>,
    audio_service: Option<AudioService>,
    input_service: Option<InputService>,
}

impl WsBackend {
    pub(crate) async fn new(
        _conn_params: &ConnectionParams,
        http_client: &HttpClient,
        streaming_config: &StreamingSessionConfig,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        info!("Start websocket session");

        // Start VideoWebsocket service
        let cursor_format = if let Some(input_config) = streaming_config.input.as_ref() {
            get_cursor_format(&input_config.pipeline)
        } else {
            None
        };

        host_service::start_stream(http_client, streaming_config, cursor_format).await?;

        // Start video service
        let video_service = if let Some(video_config) = &streaming_config.video {
            let video_service =
                Self::create_video_service(http_client, video_config, msg_proxy.clone()).await?;

            Some(video_service)
        } else {
            None
        };

        // Start audio service
        let audio_service = if streaming_config.audio {
            let audio_service = Self::create_audio_service(http_client, msg_proxy.clone()).await?;

            Some(audio_service)
        } else {
            None
        };

        // Start input service
        let input_service = if let Some(input_config) = streaming_config.input.as_ref() {
            let input_service =
                Self::create_input_service(http_client, input_config.pipeline.clone(), msg_proxy)?;

            Some(input_service)
        } else {
            None
        };

        Ok(Self {
            video_service,
            audio_service,
            input_service,
        })
    }

    async fn create_video_service(
        http_client: &HttpClient,
        video_config: &VideoSessionConfig,
        msg_proxy: MessageProxy,
    ) -> Result<VideoService> {
        let video_decoder_config = get_video_decoder_config(video_config).await?;

        let player = Rc::new(VideoPlayer::new(
            &video_config.player.canvas,
            msg_proxy.clone(),
            video_decoder_config,
        )?);

        let url = format!("wss://{host}/websocket/video", host = http_client.host());
        let ws = VideoWsSocketHandler::new(&url, player.clone())?;

        Ok(VideoService { ws, player })
    }

    async fn create_audio_service(
        http_client: &HttpClient,
        msg_proxy: MessageProxy,
    ) -> Result<AudioService> {
        let player = Rc::new(AudioPlayer::new(msg_proxy).await?);

        let url = format!("wss://{host}/websocket/audio", host = http_client.host());
        let ws = AudioWsSocketHandler::new(&url, player.clone())?;

        Ok(AudioService { ws, player })
    }

    fn create_input_service(
        http_client: &HttpClient,
        input_pipeline: Arc<InputPipeline>,
        msg_proxy: MessageProxy,
    ) -> Result<InputService> {
        // Create and configure libkynput network proxy
        let mut input_proxy = BufferProxy::new();
        let (libkynput_tx, libkynput_rx) = input_proxy.split()?;
        let input_proxy = Arc::new(input_proxy);

        input_proxy.plug_consumer(Arc::downgrade(&input_pipeline.get_input_sink()))?;
        input_proxy.run()?;

        // Spawn websocket
        let msg_sink = Arc::new(InputMsgSink::new(msg_proxy.clone()));
        let inputs_url = format!("wss://{host}/websocket/inputs", host = http_client.host());
        let input_websocket =
            InputWebSocketHandler::new(&inputs_url, libkynput_tx, libkynput_rx, msg_sink)?;

        Ok(InputService {
            websocket: input_websocket,
            proxy: input_proxy,
            pipeline: input_pipeline,
        })
    }
}

#[async_trait(?Send)]
impl BackendDriver for WsBackend {
    async fn stop(&mut self) -> Result<()> {
        if let Some(video_service) = self.video_service.take() {
            video_service.stop().await;
        }

        if let Some(audio_service) = self.audio_service.take() {
            audio_service.stop().await;
        }

        if let Some(input_service) = self.input_service.take() {
            input_service.stop();
        }

        Ok(())
    }

    async fn input_service_connected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service
            .pipeline
            .plug_network(input_service.proxy.clone())?;
        input_service.pipeline.start()?;

        Ok(())
    }

    async fn input_service_disconnected(&mut self) -> Result<()> {
        Ok(())
    }
}
