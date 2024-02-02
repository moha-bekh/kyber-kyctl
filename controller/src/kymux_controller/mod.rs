pub(crate) mod requests;
mod server;
mod webtransport;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use log::{info, warn};
use tokio::sync::Mutex;

use libkypc::process::ProcessFactory;

pub(crate) use server::KymuxServer;

use crate::avservice;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::utils::TypeConvert;
use crate::{Controller, DataPlaneBackend, StateMachineProxy};

use requests::{KymuxTransportBackend, StartKymuxCertificateHash};

pub(crate) struct KymuxBackend {
    pub(crate) server: KymuxServer,
    pub(crate) connection: Option<kymux::Connection>,
    statemachine_proxy: StateMachineProxy,
}

impl KymuxBackend {
    fn new(server: KymuxServer, statemachine_proxy: StateMachineProxy) -> Self {
        Self {
            server,
            connection: None,
            statemachine_proxy,
        }
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        if let Some(connection) = self.connection.take() {
            info!("Stop running Kymux Connection");
            connection.stop().await?;
        };

        info!("Stop running Kymux Server");
        let ret = self.server.stop().await;
        if let Err(err) = ret {
            warn!("Kymux server stop raised an error: {err:?}");
        }

        Ok(())
    }
}

async fn start_kymux(
    ctrl: web::Data<Mutex<Controller>>,
    backend: KymuxTransportBackend,
    public_port: u16,
    statemachine_proxy: StateMachineProxy,
) -> Result<requests::StartKymuxResponse> {
    {
        let ctrl = ctrl.lock().await;
        if ctrl.has_data_plane()? {
            return Err(Error::DataPlaneAlreadyOpened);
        }
    }

    // Load TLS config
    let (listening_port, tls_config) = {
        let ctrl = ctrl.lock().await;
        (ctrl.config.port(), ctrl.tls_config.clone())
    };

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listening_port);

    // Start Listen task
    let (server, certificate_hash) = match backend {
        KymuxTransportBackend::Quic => {
            info!("Start Quic listener on address {addr:?}");

            let certificate = rustls::Certificate(tls_config.certificate.clone());
            let private_key = rustls::PrivateKey(tls_config.private_key.clone());

            let config = kymux::ServerConfig::Quic {
                addr,
                certificate,
                private_key,
            };
            let server = kymux::Server::new(config).await?;
            (server, None)
        }
        KymuxTransportBackend::Webtransport => {
            info!("Start webtransport listener on address {addr:?}");

            let cert = webtransport::gen_webtransport_cert()?;

            let server_config = kymux::ServerConfig::Wtransport {
                addr,
                certificate: cert.cert_der,
                private_key: cert.key_der,
            };
            let server = kymux::Server::new(server_config).await?;

            (
                server,
                Some(StartKymuxCertificateHash {
                    algorithm: "sha-256".into(),
                    hash: cert.cert_digest,
                }),
            )
        }
    };

    // Start Listen task wrapper
    let server = KymuxServer::new(server).await?;

    {
        let mut ctrl = ctrl.lock().await;
        let Some(client_session) = ctrl.client_session.as_mut() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        let backend = KymuxBackend::new(server, statemachine_proxy);
        client_session.data_plane = Some(DataPlaneBackend::Kymux(backend));
    }

    Ok(requests::StartKymuxResponse {
        port: public_port,
        certificate_hash,
    })
}

async fn start_video(
    process_factory: &Arc<ProcessFactory>,
    statemachine_proxy: StateMachineProxy,
    display_id: u32,
    uri: String,
    bitrate: u32,
    codec: avservice_types::VideoCodec,
    yuv444: bool,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new(avservice::HandlerType::Video, statemachine_proxy);

    handler
        .kymux_start_video(process_factory, display_id, uri, bitrate, codec, yuv444)
        .await?;

    Ok(handler)
}

async fn start_audio(
    process_factory: &Arc<ProcessFactory>,
    statemachine_proxy: StateMachineProxy,
    uri: String,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new(avservice::HandlerType::Audio, statemachine_proxy);

    handler.kymux_start_audio(process_factory, uri).await?;

    Ok(handler)
}

async fn start_input(
    process_factory: &Arc<ProcessFactory>,
    host_config: inputservice_types::HostConfig,
    service_config: inputservice_types::ServiceConfig,
    uri: String,
) -> Result<inputservice::Handler> {
    let mut inputservice = inputservice::Handler::new();

    inputservice
        .start_kymux(process_factory, host_config, service_config, uri)
        .await?;

    Ok(inputservice)
}

pub(crate) struct StartSessionResponse {
    pub(crate) avservice_video: Option<avservice::Handler>,
    pub(crate) video_endpoint: Option<u16>,

    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) audio_endpoint: Option<u16>,

    pub(crate) inputservice: Option<inputservice::Handler>,
    pub(crate) input_endpoint: Option<u16>,
}

async fn start_session(
    ctrl: &mut Controller,
    request: requests::StartRequest,
) -> Result<StartSessionResponse> {
    let state_machine_proxy = ctrl.state_machine_proxy()?;

    // Complete Kymux connection
    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let Some(DataPlaneBackend::Kymux(backend)) = client_session.data_plane.as_mut() else {
        return Err(Error::KymuxNotListening);
    };

    backend.connection = Some(
        backend
            .server
            .wait_connected(Duration::from_millis(500))
            .await?,
    );

    let Some(connection) = backend.connection.as_mut() else {
        warn!("Kymux not connected");
        return Err(Error::KymuxNotConnected);
    };

    // Start services
    let (video_endpoint, avservice_video) = if let Some(video_request) = request.video {
        // Video Server
        let video_protocol = request
            .kymux_config
            .video_protocol
            .unwrap_or(kymux::VideoProtocol::Reliable);

        let (video_endpoint, video_uri) = connection
            .register_video_endpoint_with_ipc_forward(None, video_protocol)
            .await?;

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {video_endpoint:X}");

        let avservice_video = start_video(
            &ctrl.process_factory,
            state_machine_proxy.clone(),
            video_request.display_id,
            video_uri,
            request.bitrate,
            video_request.codec,
            video_request.yuv444,
        )
        .await?;

        (Some(video_endpoint), Some(avservice_video))
    } else {
        (None, None)
    };

    // Audio server
    let (audio_endpoint, avservice_audio) = if request.audio {
        let (audio_endpoint, audio_uri) = connection
            .register_audio_endpoint_with_ipc_forward(None)
            .await?;

        info!("Start Audio avservice. Endpoint: {audio_endpoint:X}");

        let avservice_audio = start_audio(
            &ctrl.process_factory,
            state_machine_proxy.clone(),
            audio_uri,
        )
        .await?;
        (Some(audio_endpoint), Some(avservice_audio))
    } else {
        warn!("Audio disabled");
        (None, None)
    };

    // Input server
    let (input_endpoint, inputservice) = if let Some(inputs_request) = request.inputs {
        let (input_endpoint, uri) = connection
            .register_input_endpoint_with_ipc_forward(None)
            .await?;

        info!("Start Inputservice. Endpoint: {input_endpoint:X}");

        let host_config = inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        let service_config = inputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format,
        };

        let inputservice =
            start_input(&ctrl.process_factory, host_config, service_config, uri).await?;
        (Some(input_endpoint), Some(inputservice))
    } else {
        warn!("Input server disabled");
        (None, None)
    };

    Ok(StartSessionResponse {
        avservice_video,
        video_endpoint,
        avservice_audio,
        audio_endpoint,
        inputservice,
        input_endpoint,
    })
}
