use std::net::SocketAddr;

use log::{error, info, warn};
use serde::Serialize;

use libkynput_rs::Pipeline;

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{input_service, tls};
use crate::{
    Display, InputSessionConfig, MessageProxy, PlayerConfig, StartStreamResult,
    StreamingSessionConfig, WindowHandle,
};

pub(crate) mod host_service;
pub(crate) mod player;

pub(crate) use player::Player;

struct VideoConfig {
    pub(self) window_handle: WindowHandle,
    pub(self) uri: String,
}

struct AudioConfig {
    pub(self) uri: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct KymuxConfig {
    pub video_protocol: Option<kymux::VideoProtocol>,
}

pub(crate) async fn start_kymux(
    addr: &SocketAddr,
    streamer_tls_host: &str,
    tls_config: &tls::Config,
    client: &HttpClient,
) -> Result<kymux::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(client, addr.port()).await?;

    // Start Kymux connection
    let mut certs_store = rustls::RootCertStore::empty();
    certs_store
        .add(&rustls::Certificate(tls_config.certificate.clone()))
        .map_err(|err| {
            warn!("Failed to crate RootCertStore: {err:?}");
            Error::Rejected
        })?;

    let kymux_config = kymux::ClientConfig::new(
        SocketAddr::new(addr.ip(), start_response.port),
        certs_store,
        streamer_tls_host,
    );

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
) -> Result<Option<VideoConfig>> {
    let config = match (player_config.window_handle, endpoint) {
        (Some(window_handle), Some(endpoint)) => {
            let uri = kymux.get_uri_for_endpoint(*endpoint).map_err(|err| {
                error!("Fail to get URI for Video endpoint {endpoint}");
                Error::Kymux(err)
            })?;

            Some(VideoConfig { window_handle, uri })
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
) -> Result<Option<AudioConfig>> {
    let config = match (enabled, endpoint) {
        (true, Some(endpoint)) => {
            let uri = kymux.get_uri_for_endpoint(*endpoint).map_err(|err| {
                error!("Fail to get URI for audio endpoint {endpoint}");
                Error::Kymux(err)
            })?;

            Some(AudioConfig { uri })
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
) -> Result<(Option<input_service::Client>, Option<Pipeline>)> {
    let input_uri = kymux.get_uri_for_endpoint(endpoint).map_err(|err| {
        error!("Fail to get URI for input endpoint {endpoint}");
        Error::Kymux(err)
    })?;

    info!("Start input. URI: {input_uri}");

    let create_stream_cb =
        |target, msg_sink| libkynput_rs::kymux::new_stream(target, msg_sink, &input_uri);

    let input_service = input_service::Client::new(
        msg_proxy,
        config.pipeline.get_input_sink(),
        create_stream_cb,
    )?;

    Ok((Some(input_service), Some(config.pipeline.clone())))
}

pub(crate) async fn start(
    client: &HttpClient,
    displays: &Vec<Display>,
    player_config: &PlayerConfig,
    streaming_config: &StreamingSessionConfig,
    kymux_config: KymuxConfig,
    kymux: &mut kymux::Connection,
    msg_proxy: MessageProxy,
) -> Result<StartStreamResult> {
    // Start stream
    let start_resp = host_service::start_stream(
        client,
        displays,
        streaming_config.bitrate,
        kymux_config,
        &streaming_config.video,
        streaming_config.audio,
        streaming_config.input.is_some(),
    )
    .await?;

    // Play Video and Audio
    let video_config = create_video_config(kymux, player_config, &start_resp.video_endpoint)?;

    let audio_config =
        create_audio_config(kymux, streaming_config.audio, &start_resp.audio_endpoint)?;

    let player = if video_config.is_some() || audio_config.is_some() {
        let mut player = player::Player::new(msg_proxy.clone())?;
        player.play(video_config, audio_config)?;

        Some(crate::Player::Kymux(player))
    } else {
        None
    };

    // Start input
    let (input_service, input_pipeline) = match (
        streaming_config.input.as_ref(),
        start_resp.input_endpoint,
    ) {
        (Some(config), Some(endpoint)) => start_input(kymux, config, endpoint, msg_proxy.clone())?,
        (None, None) => (None, None),
        _ => {
            error!("Invalid Input config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    Ok(StartStreamResult {
        player,
        input_service,
        input_pipeline,
    })
}
