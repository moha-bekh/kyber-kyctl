use std::net::SocketAddr;

use async_trait::async_trait;
use log::{error, info, warn};
use serde::Serialize;

use libkynput_rs::Pipeline;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{input_service, player};
use crate::{
    BackendDriver, ConnectionParams, Display, InputSessionConfig, MessageProxy, PlayerConfig,
    StreamingSessionConfig,
};

pub(crate) mod host_service;

#[derive(Clone, Debug, Serialize)]
pub struct KymuxConfig {
    pub video_protocol: Option<kymux::VideoProtocol>,
}

async fn start_kymux(
    conn_params: &ConnectionParams,
    client: &HttpClient,
) -> Result<kymux::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(client, conn_params.port).await?;

    // Start Kymux connection
    let mut certs_store = rustls::RootCertStore::empty();
    certs_store
        .add(&rustls::Certificate(
            conn_params.tls_config.root_certificate.clone(),
        ))
        .map_err(|err| {
            warn!("Failed to crate RootCertStore: {err:?}");
            Error::Rejected
        })?;

    let addr = SocketAddr::new(conn_params.addr.ip(), start_response.port);

    let kymux_config = kymux::ClientConfig::new(addr, certs_store, &conn_params.host.clone());

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
    player_config: &PlayerConfig,
    endpoint: &Option<u16>,
) -> Result<Option<player::VideoConfig>> {
    let config = match (player_config.window_handle, endpoint) {
        (Some(window_handle), Some(endpoint)) => {
            let uri = kymux.get_uri_for_endpoint(*endpoint).map_err(|err| {
                error!("Fail to get URI for Video endpoint {endpoint}");
                Error::Kymux(err)
            })?;

            Some(player::VideoConfig { window_handle, uri })
        }
        (None, None) => None,
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
            let uri = kymux.get_uri_for_endpoint(*endpoint).map_err(|err| {
                error!("Fail to get URI for audio endpoint {endpoint}");
                Error::Kymux(err)
            })?;

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

fn start_input(
    kymux: &kymux::Connection,
    config: &InputSessionConfig,
    endpoint: u16,
    msg_proxy: MessageProxy,
) -> Result<Option<InputService>> {
    let input_uri = kymux.get_uri_for_endpoint(endpoint).map_err(|err| {
        error!("Fail to get URI for input endpoint {endpoint}");
        Error::Kymux(err)
    })?;

    info!("Start input. URI: {input_uri}");

    let create_stream_cb =
        |target, msg_sink| libkynput_rs::kymux::new_stream(target, msg_sink, &input_uri);

    let client = input_service::Client::new(
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
    client: input_service::Client,
    pipeline: Pipeline,
}

pub(crate) struct KymuxBackend {
    kymux: kymux::Connection,
    player: Option<player::Player>,
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
        let kymux = start_kymux(conn_params, http_client).await?;

        let cursor_format = if let Some(input_config) = streaming_config.input.as_ref() {
            input_service::get_cursor_format(&input_config.pipeline)
        } else {
            None
        };

        // Start stream
        let start_resp = host_service::start_stream(
            http_client,
            displays,
            streaming_config.bitrate,
            kymux_config,
            &streaming_config.video,
            streaming_config.audio,
            cursor_format,
        )
        .await?;

        // Play Video and Audio
        let video_config = create_video_config(&kymux, player_config, &start_resp.video_endpoint)?;

        let audio_config =
            create_audio_config(&kymux, streaming_config.audio, &start_resp.audio_endpoint)?;

        let player = if video_config.is_some() || audio_config.is_some() {
            let mut player = player::Player::new(player_config.software_decode, msg_proxy.clone())?;
            player.play(video_config, audio_config)?;

            Some(player)
        } else {
            None
        };

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
            player.stop_async().await?;
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
