mod audio;
mod host_service;
mod input;
mod packet;
mod video;

use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};

use libkynput::bufferproxy::BufferProxy;
use libkynput::InputNetworkStream;

use audio::AudioWsSocketHandler;
use input::InputWebSocketHandler;
use video::VideoWsSocketHandler;

use crate::platform::player::{
    get_video_decoder_config, AudioPlayer, AudioPlayerHandle, VideoPlayer, VideoPlayerHandle,
};
use crate::platform::AudioPlayerConfig;
use crate::platform::InputMsgSink;

use crate::http::HttpClient;
use crate::{BackendDriver, MessageProxy, StreamingSessionConfig};
use crate::{Error, Result};

struct VideoService {
    ws: Rc<VideoWsSocketHandler>,
    player_handle: VideoPlayerHandle,
}

impl VideoService {
    async fn stop(mut self) {
        info!("Stop video service");

        let ret = self.ws.stop();
        if let Err(err) = ret {
            warn!("Video ws stop failed: {err:?}");
        }

        let ret = self.player_handle.stop().await;
        if let Err(err) = ret {
            warn!("Video player stop failed: {err:?}");
        }
    }
}

struct AudioService {
    ws: Rc<AudioWsSocketHandler>,
    player_handle: AudioPlayerHandle,
}

impl AudioService {
    async fn stop(mut self) {
        info!("Stop audio service");

        let ret = self.ws.stop();
        if let Err(err) = ret {
            warn!("Audio ws stop failed: {err:?}");
        }

        let ret = self.player_handle.stop().await;
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

pub(crate) struct WsBackendConfig<'a> {
    pub(crate) http_client: &'a HttpClient,
    pub(crate) streaming_config: &'a StreamingSessionConfig,
    pub(crate) msg_proxy: MessageProxy,
}

pub(crate) struct WsBackend {
    video_service: Option<VideoService>,
    audio_service: Option<AudioService>,
    input_service: Option<InputService>,
}

impl WsBackend {
    pub(crate) async fn new(backend_config: WsBackendConfig<'_>) -> Result<Self> {
        info!("Start websocket session");

        // Start Websocket service
        host_service::start_stream(&backend_config).await?;

        // Start services
        let video_service = Self::create_video_service(&backend_config).await?;
        let audio_service = Self::create_audio_service(&backend_config).await?;
        let input_service = Self::create_input_service(&backend_config)?;

        Ok(Self {
            video_service,
            audio_service,
            input_service,
        })
    }

    async fn create_video_service(
        backend_config: &WsBackendConfig<'_>,
    ) -> Result<Option<VideoService>> {
        let video_config = &backend_config.streaming_config.video;
        if video_config.is_empty() {
            return Ok(None);
        }

        let video_config = &video_config[0];

        let video_decoder_config = get_video_decoder_config(video_config).await?;

        let (player, player_handle) = VideoPlayer::create(
            &video_config.player.canvas,
            backend_config.msg_proxy.clone(),
            video_decoder_config,
        )?;

        let url = format!(
            "wss://{host}/websocket/video",
            host = backend_config.http_client.host()
        );
        let ws = VideoWsSocketHandler::new(&url, player)?;

        Ok(Some(VideoService { ws, player_handle }))
    }

    async fn create_audio_service(
        backend_config: &WsBackendConfig<'_>,
    ) -> Result<Option<AudioService>> {
        let Some(audio_config) = &backend_config.streaming_config.audio else {
            return Ok(None);
        };

        let audio_buffer = audio_config.audio_buffer();
        let audio_player_config = AudioPlayerConfig { audio_buffer };

        let (player, player_handle) =
            AudioPlayer::create(backend_config.msg_proxy.clone(), &audio_player_config).await?;

        let url = format!(
            "wss://{host}/websocket/audio",
            host = backend_config.http_client.host()
        );
        let ws = AudioWsSocketHandler::new(&url, player)?;

        Ok(Some(AudioService { ws, player_handle }))
    }

    fn create_input_service(backend_config: &WsBackendConfig<'_>) -> Result<Option<InputService>> {
        let Some(input_config) = backend_config.streaming_config.input.as_ref() else {
            return Ok(None);
        };

        // Create and configure libkynput network proxy
        let mut input_proxy = BufferProxy::new();
        let (libkynput_tx, libkynput_rx) = input_proxy.split()?;
        #[allow(clippy::arc_with_non_send_sync)]
        let input_proxy = Arc::new(input_proxy);

        input_proxy.plug_consumer(Arc::downgrade(&input_config.pipeline.get_input_sink()))?;
        input_proxy.run()?;

        // Spawn websocket
        #[allow(clippy::arc_with_non_send_sync)]
        let msg_sink = Arc::new(InputMsgSink::new(backend_config.msg_proxy.clone()));
        let inputs_url = format!(
            "wss://{host}/websocket/inputs",
            host = backend_config.http_client.host()
        );
        let input_websocket =
            InputWebSocketHandler::new(&inputs_url, libkynput_tx, libkynput_rx, msg_sink)?;

        Ok(Some(InputService {
            websocket: input_websocket,
            proxy: input_proxy,
            pipeline: input_config.pipeline.clone(),
        }))
    }
}

#[async_trait(?Send)]
impl BackendDriver for WsBackend {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let audio_service = self.audio_service.as_ref().ok_or(Error::NoAudioPlayer)?;

        audio_service.player_handle.set_volume(volume)?;

        Ok(())
    }

    fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let audio_service = self.audio_service.as_ref().ok_or(Error::NoAudioPlayer)?;

        audio_service.player_handle.set_mute(mute)?;

        Ok(())
    }

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
