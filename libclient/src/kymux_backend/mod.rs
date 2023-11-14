use std::net::SocketAddr;

use async_trait::async_trait;
use log::{error, info, warn};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::platform::player;
use crate::platform::{get_cursor_format, InputClient, InputPipeline};
use crate::{
    BackendDriver, ConnectionParams, Display, InputSessionConfig, MessageProxy, PlayerConfig,
    StreamingSessionConfig,
};

pub(crate) mod host_service;

#[derive(Clone, Copy, Debug, Serialize)]
pub enum KymuxTransportBackend {
    #[cfg(feature = "desktop")]
    Quic,
}

#[derive(Clone, Debug, Serialize)]
pub struct KymuxConfig {
    pub backend: KymuxTransportBackend,
    pub video_protocol: Option<kymux::VideoProtocol>,
}

struct Player(player::Player);

impl Player {
    async fn stop(mut self) -> Result<()> {
        info!("Stop player");
        self.0.stop_async().await?;
        info!("Player stopped");

        Ok(())
    }
}

async fn start_kymux(
    conn_params: &ConnectionParams,
    client: &HttpClient,
    backend: KymuxTransportBackend,
) -> Result<kymux::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(client, backend, conn_params.port).await?;

    // Start Kymux connection
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(&rustls::Certificate(
            conn_params.tls_config.root_certificate.clone(),
        ))
        .map_err(|err| {
            warn!("Failed to crate RootCertStore: {err:?}");
            Error::Rejected
        })?;

    let addr = SocketAddr::new(conn_params.addr.ip(), start_response.port);

    let kymux_config = kymux::ClientConfig::Quic {
        addr,
        roots,
        server_name: conn_params.host.clone(),
    };

    let kymux = kymux::Connection::connect(kymux_config)
        .await
        .map_err(|err| {
            error!("Fail to connect to Kymux: {err:?}");
            Error::Kymux(err)
        })?;

    info!("kymux: Connected to {addr}");

    Ok(kymux)
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

async fn create_player(
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

fn start_input(
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

struct InputService {
    client: InputClient,
    pipeline: InputPipeline,
}

pub(crate) struct KymuxBackend {
    kymux: kymux::Connection,
    player: Option<Player>,
    input_service: Option<InputService>,
}

impl KymuxBackend {
    pub(crate) async fn new(
        conn_params: &ConnectionParams,
        http_client: &HttpClient,
        displays: &Vec<Display>,
        player_config: &PlayerConfig,
        streaming_config: &StreamingSessionConfig,
        kymux_config: KymuxConfig,
        msg_proxy: MessageProxy,
    ) -> Result<Self> {
        let mut kymux = start_kymux(conn_params, http_client, kymux_config.backend).await?;

        let cursor_format = if let Some(input_config) = streaming_config.input.as_ref() {
            get_cursor_format(&input_config.pipeline)
        } else {
            None
        };

        // Start stream
        let start_resp = host_service::start_stream(
            http_client,
            displays,
            streaming_config.bitrate,
            kymux_config.clone(),
            &streaming_config.video,
            streaming_config.audio,
            cursor_format,
        )
        .await?;

        // Play Video and Audio
        let player = create_player(
            &mut kymux,
            &kymux_config,
            player_config,
            streaming_config,
            &start_resp.video_endpoint,
            &start_resp.audio_endpoint,
            msg_proxy.clone(),
        )
        .await?;

        // Start input
        let input_service = match (streaming_config.input.as_ref(), start_resp.input_endpoint) {
            (Some(config), Some(endpoint)) => {
                start_input(&kymux, config, endpoint, msg_proxy.clone())?
            }
            (None, None) => None,
            _ => {
                error!("Invalid Input config. Should either get a config and an endpoint, or none of them.");
                return Err(Error::InvalidState);
            }
        };

        Ok(Self {
            kymux,
            player,
            input_service,
        })
    }
}

#[async_trait]
impl BackendDriver for KymuxBackend {
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
        if let Some(mut player) = self.player.take() {
            player.0.stop_async().await?;
        }

        if let Some(input_service) = self.input_service.take() {
            info!("Stop input service");
            input_service.pipeline.stop()?;
        }

        let ret = self.kymux.stop().await;
        match ret {
            Ok(_) => {
                info!("Kymux stopped");
            }
            Err(err) => {
                warn!("Failed to stop kymux: {err:?}");
            }
        }

        Ok(())
    }
}
