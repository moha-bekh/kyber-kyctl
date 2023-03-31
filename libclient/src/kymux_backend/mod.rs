use std::net::SocketAddr;

use log::{error, info, warn};

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::{input_service, tls};
use crate::{ClientConfig, MessageProxy};

pub(crate) mod host_service;
pub(crate) mod player;

pub(crate) use player::Player;

pub(crate) struct StartResult {
    pub(crate) player: Player,
    pub(crate) input_service: Option<input_service::Client>,
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

pub(crate) async fn start(
    config: &ClientConfig,
    client: &HttpClient,
    kymux: &mut kymux::Connection,
    msg_proxy: MessageProxy,
) -> Result<StartResult> {
    // Start stream
    let start_resp = host_service::start_stream(
        client,
        config.video_codec,
        config.yuv444,
        config.audio,
        config.inputs,
    )
    .await?;

    // Play Video and Audio
    let mut player = player::Player::new(config.window_handle, msg_proxy.clone())?;
    let video_uri = kymux
        .get_uri_for_endpoint(start_resp.video_endpoint)
        .map_err(|err| {
            error!(
                "Fail to get URI for Video endpoint {}",
                start_resp.video_endpoint
            );
            Error::Kymux(err)
        })?;

    let audio_uri = if let Some(audio_endpoint) = start_resp.audio_endpoint {
        let uri = kymux.get_uri_for_endpoint(audio_endpoint).map_err(|err| {
            error!("Fail to get URI for audio endpoint {audio_endpoint}");
            Error::Kymux(err)
        })?;

        Some(uri)
    } else {
        None
    };

    player.play(video_uri, audio_uri)?;

    // Start input
    let input_service = if let Some(input_endpoint) = start_resp.input_endpoint {
        let input_uri = kymux.get_uri_for_endpoint(input_endpoint).map_err(|err| {
            error!("Fail to get URI for input endpoint {input_endpoint}");
            Error::Kymux(err)
        })?;

        info!("Start input. URI: {input_uri}");

        let create_stream_cb =
            |target, msg_sink| libkynput_rs::kymux::new_stream(target, msg_sink, &input_uri);

        let input_service = input_service::Client::new(
            msg_proxy,
            config.input_pipeline.get_input_sink(),
            create_stream_cb,
        )?;

        Some(input_service)
    } else {
        None
    };

    Ok(StartResult {
        player,
        input_service,
    })
}
