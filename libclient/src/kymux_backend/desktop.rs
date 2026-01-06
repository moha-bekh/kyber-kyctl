use async_trait::async_trait;
use log::info;

use kymux::{kycom, kyproto};

use super::{KymuxBackendConfig, PlatformEndpoints};
use crate::platform::player;
use crate::platform::{InputClient, InputPipeline};
use crate::{BackendDriver, MessageSender};
use crate::{Error, Result};

struct Player(player::Player);

impl Player {
    async fn stop(mut self) -> Result<()> {
        info!("Stop player");
        self.0.stop_async().await?;
        info!("Player stopped");

        Ok(())
    }
}

fn create_video_config(
    kycom: &mut kycom::KyCom,
    backend_config: &KymuxBackendConfig<'_>,
    endpoints: Vec<kyproto::VideoClientEndpoint>,
) -> Result<Option<Vec<player::VideoConfig>>> {
    if endpoints.is_empty() {
        return Ok(None);
    };

    let video_config = &backend_config.streaming_config.video;

    assert!(!video_config.is_empty(), "called without video config");

    // By construction, one endpoint is created for each video_config.
    // If this is not the case, this is a programming error.
    assert_eq!(
        endpoints.len(),
        video_config.len(),
        "Mismatch between video_config and endpoints"
    );

    let mut player_config_list = vec![];

    for (config, endpoint) in video_config.iter().zip(endpoints.into_iter()) {
        let uri = kycom.register_and_forward(endpoint)?.url();

        let player_config = player::VideoConfig {
            source_id: config.display_id,
            software_decode: config.player.software_decode,
            window_handle: config.player.window_handle,
            uri,
            video_buffer: config.player.video_buffer,
        };

        player_config_list.push(player_config);
    }

    Ok(Some(player_config_list))
}

fn create_audio_config(
    kycom: &mut kycom::KyCom,
    backend_config: &KymuxBackendConfig<'_>,
    endpoint: Option<kyproto::AudioClientEndpoint>,
) -> Result<Option<player::AudioConfig>> {
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };

    let audio_config = backend_config
        .streaming_config
        .audio
        .as_ref()
        .expect("called without audio config");

    let uri = kycom.register_and_forward(endpoint)?.url();
    let player_config = player::AudioConfig {
        uri,
        audio_buffer: audio_config.audio_buffer(),
    };

    Ok(Some(player_config))
}

async fn create_player(
    kycom: &mut kycom::KyCom,
    backend_config: &KymuxBackendConfig<'_>,
    metrics: bool,
    video_endpoints: Vec<kyproto::VideoClientEndpoint>,
    audio_endpoint: Option<kyproto::AudioClientEndpoint>,
) -> Result<Option<Player>> {
    let video_config = create_video_config(kycom, backend_config, video_endpoints)?;

    let audio_config = create_audio_config(kycom, backend_config, audio_endpoint)?;

    let player = if video_config.is_some() || audio_config.is_some() {
        let audio_buffer = audio_config.as_ref().map(|config| config.audio_buffer);
        let video_buffer = if let Some(video_config) = video_config.as_ref() {
            // XXX VLC only supports one global video_buffer value, but each
            // VideoPlayerConfig has a video_buffer value
            if video_config.is_empty() {
                None
            } else {
                Some(video_config[0].video_buffer)
            }
        } else {
            None
        };

        let mut player = player::Player::new(
            backend_config.msg_sender.clone(),
            video_buffer,
            audio_buffer,
            metrics,
        )?;
        player.play(video_config, audio_config)?;

        Some(Player(player))
    } else {
        None
    };

    Ok(player)
}

async fn start_input(
    kycom: &mut kycom::KyCom,
    backend_config: &KymuxBackendConfig<'_>,
    msg_sender: MessageSender,
    endpoint: Option<kyproto::InputEndpoint>,
) -> Result<Option<InputService>> {
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };

    let input_config = backend_config
        .streaming_config
        .input
        .as_ref()
        .expect("called without input config");

    let input_uri = kycom.register_and_forward(endpoint)?.url();

    info!("Start input. URI: {input_uri}");

    let create_stream_cb =
        |target, msg_sink| libkynput_rs::kymux::new_stream(target, msg_sink, &input_uri);

    let client = InputClient::new(
        msg_sender,
        input_config.pipeline.get_input_sink(),
        create_stream_cb,
    )?;

    Ok(Some(InputService {
        client,
        pipeline: input_config.pipeline.clone(),
    }))
}

struct InputService {
    pipeline: InputPipeline,
    client: InputClient,
}

pub(super) struct KymuxBackend {
    kycom: Option<kycom::KyCom>,
    player: Option<Player>,
    input_service: Option<InputService>,
}

impl KymuxBackend {
    pub(super) async fn new(
        backend_config: &KymuxBackendConfig<'_>,
        endpoints: PlatformEndpoints,
        has_metrics: bool,
    ) -> Result<Self> {
        const IPC_PORTS: std::ops::Range<u16> = 9090..9100;
        let mut kycom = kycom::KyCom::start_on_any_port(IPC_PORTS).await?;

        // Play Video and Audio
        let player = create_player(
            &mut kycom,
            backend_config,
            has_metrics,
            endpoints.video,
            endpoints.audio,
        )
        .await?;

        // Start input
        let input_service = start_input(
            &mut kycom,
            backend_config,
            backend_config.msg_sender.clone(),
            endpoints.input,
        )
        .await?;

        Ok(Self {
            kycom: Some(kycom),
            player,
            input_service,
        })
    }
}

#[async_trait]
impl BackendDriver for KymuxBackend {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let player = self.player.as_ref().ok_or(Error::NoAudioPlayer)?;

        player.0.audio_set_volume(volume)?;

        Ok(())
    }

    fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let player = self.player.as_ref().ok_or(Error::NoAudioPlayer)?;

        player.0.audio_set_mute(mute)?;

        Ok(())
    }

    async fn input_service_connected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service
            .pipeline
            .plug_network(&input_service.client.get_tx_consumer())?;
        input_service.pipeline.start()?;

        Ok(())
    }

    async fn input_service_disconnected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service.pipeline.stop()?;

        self.input_service = None;

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(player) = self.player.take() {
            player.stop().await?;
        }

        if let Some(input_service) = self.input_service.take() {
            input_service.pipeline.stop()?;
        }

        if let Some(kycom) = self.kycom.take() {
            kycom.stop();
        }

        Ok(())
    }
}
