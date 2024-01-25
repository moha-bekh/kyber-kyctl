pub(crate) mod requests;
mod webtransport;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use log::{info, warn};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{self, JoinHandle};

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::utils::TypeConvert;
use crate::Controller;

use requests::{KymuxTransportBackend, StartKymuxCertificateHash};

struct KymuxServerTaskRx {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,

    connected_rx: oneshot::Receiver<kymux::Connection>,
}

pub(crate) struct KymuxServer {
    _server: Arc<kymux::Server>,
    task_rx: Option<KymuxServerTaskRx>,
}

impl KymuxServer {
    pub(crate) async fn new(server: kymux::Server) -> Result<Self> {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (connected_tx, connected_rx) = oneshot::channel();

        let server = Arc::new(server);
        let server_task = server.clone();

        let join_handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                ret = Self::run_task(server_task, connected_tx) => {
                    if let Err(err) = ret {
                        warn!("KymuxServer task ended with error {err:?}");
                    }
                }
            }
        });

        Ok(Self {
            _server: server,
            task_rx: Some(KymuxServerTaskRx {
                stop_tx,
                join_handle,
                connected_rx,
            }),
        })
    }

    async fn stop_task_internal(
        stop_tx: oneshot::Sender<()>,
        join_handle: JoinHandle<()>,
    ) -> Result<()> {
        let ret = stop_tx.send(());
        if ret.is_err() {
            warn!("KymuxServer already stopped");
        }

        join_handle.await.map_err(|err| {
            warn!("KymuxServer panicked: {err:?}");
            Error::ThreadPanicked
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        if let Some(task_rx) = self.task_rx {
            Self::stop_task_internal(task_rx.stop_tx, task_rx.join_handle).await?;
        }

        Ok(())
    }

    pub(crate) async fn wait_connected(&mut self, timeout: Duration) -> Result<kymux::Connection> {
        let Some(task_rx) = self.task_rx.take() else {
            warn!("Kymux not listening");
            return Err(Error::KymuxNotListening);
        };

        let connection = match tokio::time::timeout(timeout, task_rx.connected_rx).await {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(_)) => Err(Error::ChannelClosedRx),
            Err(_) => {
                warn!("Got not Kymux connection after {timeout:?}");
                Err(Error::KymuxAcceptTimeout)
            }
        };

        let ret = Self::stop_task_internal(task_rx.stop_tx, task_rx.join_handle).await;
        if let Err(err) = ret {
            warn!("Fail to stop KymuxServer: {err:?}");
        }

        connection
    }

    async fn run_task(
        server: Arc<kymux::Server>,
        connected_tx: oneshot::Sender<kymux::Connection>,
    ) -> Result<()> {
        info!("Waiting for Kymux connection");
        let connection = server.accept().await?;

        let ret = connected_tx.send(connection);
        if ret.is_err() {
            warn!("Fail to send Kymux connection notification");
        }

        Ok(())
    }
}

async fn start_kymux(
    ctrl: web::Data<Mutex<Controller>>,
    backend: KymuxTransportBackend,
    public_port: u16,
) -> Result<requests::StartKymuxResponse> {
    // Drop existing kymux instances
    {
        let mut ctrl = ctrl.lock().await;
        crate::controller::stop_kymux(ctrl.kymux_server.take(), ctrl.kymux_connection.take())
            .await?;
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

    let mut ctrl = ctrl.lock().await;
    ctrl.kymux_server = Some(server);

    Ok(requests::StartKymuxResponse {
        port: public_port,
        certificate_hash,
    })
}

async fn start_video(
    process_factory: &Arc<ProcessFactory>,
    display_id: u32,
    uri: String,
    bitrate: u32,
    codec: avservice_types::VideoCodec,
    yuv444: bool,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new("Video");

    handler
        .kymux_start_video(process_factory, display_id, uri, bitrate, codec, yuv444)
        .await?;

    Ok(handler)
}

async fn start_audio(
    process_factory: &Arc<ProcessFactory>,
    uri: String,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new("Audio");

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
    // Complete Kymux connection
    let Some(kymux_server) = ctrl.kymux_server.as_mut() else {
        warn!("Kymux not listening");
        return Err(Error::KymuxNotListening);
    };

    ctrl.kymux_connection = Some(
        kymux_server
            .wait_connected(Duration::from_millis(500))
            .await?,
    );

    let Some(kymux_connection) = ctrl.kymux_connection.as_mut() else {
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

        let (video_endpoint, video_uri) = kymux_connection
            .register_video_endpoint_with_ipc_forward(None, video_protocol)
            .await?;

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {video_endpoint:X}");

        let avservice_video = start_video(
            &ctrl.process_factory,
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
        let (audio_endpoint, audio_uri) = kymux_connection
            .register_audio_endpoint_with_ipc_forward(None)
            .await?;

        info!("Start Audio avservice. Endpoint: {audio_endpoint:X}");

        let avservice_audio = start_audio(&ctrl.process_factory, audio_uri).await?;
        (Some(audio_endpoint), Some(avservice_audio))
    } else {
        warn!("Audio disabled");
        (None, None)
    };

    // Input server
    let (input_endpoint, inputservice) = if let Some(inputs_request) = request.inputs {
        let (input_endpoint, uri) = kymux_connection
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
