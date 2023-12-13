use std::net::SocketAddr;

use log::{error, info, warn};

use super::KymuxConfig;
use crate::platform::player;
use crate::platform::{InputClient, InputPipeline};
use crate::{
    ConnectionParams, InputSessionConfig, MessageProxy, PlayerConfig, StreamingSessionConfig,
};
use crate::{Error, Result};

pub(super) struct Player(player::Player);

impl Player {
    pub(super) async fn stop(mut self) -> Result<()> {
        info!("Stop player");
        self.0.stop_async().await?;
        info!("Player stopped");

        Ok(())
    }
}

pub(super) async fn connect_quinn(
    conn_params: &ConnectionParams,
    port: u16,
) -> Result<kymux::Connection> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(&rustls::Certificate(
            conn_params.tls_config.root_certificate.clone(),
        ))
        .map_err(|err| {
            warn!("Failed to crate RootCertStore: {err:?}");
            Error::Rejected
        })?;

    Ok(kymux::Connection::connect(kymux::ClientConfig::Quic {
        addr: SocketAddr::new(conn_params.addr.ip(), port),
        roots,
        server_name: conn_params.tls_host.clone(),
    })
    .await?)
}

fn create_video_config(
    kymux: &kymux::Connection,
    kymux_config: &KymuxConfig,
    player_config: &PlayerConfig,
    endpoint: &Option<u16>,
) -> Result<Option<player::VideoConfig>> {
    let config = match (
        player_config.window_handle,
        endpoint,
        kymux_config.video_protocol,
    ) {
        (Some(window_handle), Some(endpoint), Some(video_protocol)) => {
            let uri = kymux.connect_video_endpoint_with_ipc_forward(*endpoint, video_protocol)?;

            Some(player::VideoConfig { window_handle, uri })
        }
        (None, None, None) => None,
        _ => {
            error!("Invalid Video config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    Ok(config)
}

fn create_audio_config(
    kymux: &kymux::Connection,
    enabled: bool,
    endpoint: &Option<u16>,
) -> Result<Option<player::AudioConfig>> {
    let config = match (enabled, endpoint) {
        (true, Some(endpoint)) => {
            let uri = kymux.connect_audio_endpoint_with_ipc_forward(*endpoint)?;

            Some(player::AudioConfig { uri })
        }
        (false, None) => None,
        _ => {
            error!("Invalid Audio config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    Ok(config)
}

pub(super) async fn create_player(
    kymux: &mut kymux::Connection,
    kymux_config: &KymuxConfig,
    player_config: &PlayerConfig,
    streaming_config: &StreamingSessionConfig,
    video_endpoint: &Option<u16>,
    audio_endpoint: &Option<u16>,
    msg_proxy: MessageProxy,
) -> Result<Option<Player>> {
    let video_config = create_video_config(kymux, kymux_config, player_config, video_endpoint)?;

    let audio_config = create_audio_config(kymux, streaming_config.audio, audio_endpoint)?;

    let player = if video_config.is_some() || audio_config.is_some() {
        let mut player = player::Player::new(player_config.software_decode, msg_proxy.clone())?;
        player.play(video_config, audio_config)?;

        Some(Player(player))
    } else {
        None
    };

    Ok(player)
}

pub(super) async fn start_input(
    kymux: &kymux::Connection,
    config: &InputSessionConfig,
    endpoint: u16,
    msg_proxy: MessageProxy,
) -> Result<Option<InputService>> {
    let input_uri = kymux.connect_input_endpoint_with_ipc_forward(endpoint)?;

    info!("Start input. URI: {input_uri}");

    let create_stream_cb =
        |target, msg_sink| libkynput_rs::kymux::new_stream(target, msg_sink, &input_uri);

    let client = InputClient::new(
        msg_proxy,
        config.pipeline.get_input_sink(),
        create_stream_cb,
    )?;

    Ok(Some(InputService {
        client,
        pipeline: config.pipeline.clone(),
    }))
}

pub(super) struct InputService {
    pub(super) pipeline: InputPipeline,
    pub(super) client: InputClient,
}

pub(super) struct Inner {
    pub(super) player: Option<Player>,
    pub(super) input_service: Option<InputService>,
}

impl Inner {
    pub(super) async fn input_service_connected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service
            .pipeline
            .plug_network(&input_service.client.get_tx_consumer())?;
        input_service.pipeline.start()?;

        Ok(())
    }

    pub(super) async fn input_service_disconnected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service.pipeline.stop()?;

        self.input_service = None;

        Ok(())
    }

    pub(super) async fn stop(&mut self) -> Result<()> {
        if let Some(player) = self.player.take() {
            player.stop().await?;
        }

        if let Some(_input_service) = self.input_service.take() {
            info!("Stop input service");
        }

        Ok(())
    }
}
