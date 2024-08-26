pub(crate) mod requests;
mod server;
mod webtransport;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use log::{info, warn};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{self, JoinHandle};

use libkypc::process::ProcessFactory;

pub(crate) use server::KymuxServer;

use crate::avservice;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::utils::TypeConvert;
use crate::{Controller, DataPlaneBackend, StateMachineEvent, StateMachineProxy};

use requests::{KymuxTransportBackend, StartKymuxCertificateHash};

struct ClosedTask {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,
}

impl ClosedTask {
    fn new(connection: Arc<kymux::Connection>, statemachine_proxy: StateMachineProxy) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join_handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                ret = Self::run_task(connection, statemachine_proxy) => {
                    if let Err(err) = ret {
                        warn!("ClosedTask task ended with error {err:?}");
                    }
                }
            }
        });

        Self {
            stop_tx,
            join_handle,
        }
    }

    async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("ClosedTask already stopped");
        }

        self.join_handle.await.map_err(|err| {
            warn!("ClosedTask panicked: {err:?}");
            Error::ThreadPanicked
        })
    }

    async fn run_task(
        connection: Arc<kymux::Connection>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<()> {
        let ret = connection.closed().await;
        info!("Kymux connection closed: {ret:?}");

        statemachine_proxy
            .send(StateMachineEvent::KymuxTimeout)
            .await;

        Ok(())
    }
}

struct ConnectionWrapper {
    connection: Arc<kymux::Connection>,
    closed_task: ClosedTask,
}

pub(crate) struct KymuxBackend {
    pub(crate) server: KymuxServer,
    connection: Option<ConnectionWrapper>,
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

            let ret = connection.closed_task.stop().await;
            if let Err(err) = ret {
                warn!("Kymux Closed task stop raised an error: {err:?}");
            }

            let ret = connection.connection.stop().await;
            if let Err(err) = ret {
                warn!("Kymux Connection stop raised an error: {err:?}");
            }
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

    // Load network and TLS config
    let (listening_port, public_port, tls_config) = {
        let ctrl = ctrl.lock().await;

        // If dataplane port has been overriden, use it directly.
        // This will break controllers deployed behind a NAT.
        let (listening_port, public_port) =
            if let Some(dataplane_port) = ctrl.config.dataplane_port() {
                (dataplane_port, dataplane_port)
            } else {
                (ctrl.config.port(), public_port)
            };

        (listening_port, public_port, ctrl.tls_config.clone())
    };

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listening_port);

    // Start Listen task
    let (server, certificate_hash) = match backend {
        KymuxTransportBackend::Quic => {
            info!("Start Quic listener on address {addr:?}");

            let cert_chain = tls_config
                .cert_chain
                .iter()
                .map(|c| rustls::Certificate(c.clone()))
                .collect();
            let private_key = rustls::PrivateKey(tls_config.private_key.clone());

            let config = kymux::ServerConfig::Quic {
                addr,
                cert_chain,
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
                cert_chain: vec![rustls::Certificate(cert.cert_der)],
                private_key: rustls::PrivateKey(cert.key_der),
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
    request: &requests::StartRequest,
    connection: &mut ConnectionWrapper,
    process_factory: &Arc<ProcessFactory>,
    statemachine_proxy: StateMachineProxy,
) -> Result<Option<Vec<StartSessionVideoResponse>>> {
    let Some(video_request) = &request.video else {
        return Ok(None);
    };

    let video_stream_count = video_request.len() as u32;

    let mut video_streams = vec![];

    for stream_request in video_request {
        let video_protocol = request
            .kymux_config
            .video_protocol
            .unwrap_or(kymux::VideoProtocol::Reliable);

        let (endpoint, uri) = connection
            .connection
            .register_video_endpoint_with_ipc_forward(None, video_protocol)
            .await?;

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {endpoint:X}");
        let mut avservice =
            avservice::Handler::new(avservice::HandlerType::Video, statemachine_proxy.clone());
        avservice
            .kymux_start_video(
                process_factory,
                stream_request.display_id,
                uri,
                request.bitrate / video_stream_count,
                stream_request.codec.clone(),
                stream_request.yuv444,
                stream_request.intra_refresh,
            )
            .await?;

        video_streams.push(StartSessionVideoResponse {
            display_id: stream_request.display_id,
            avservice,
            endpoint,
        })
    }

    Ok(Some(video_streams))
}

async fn start_audio(
    request: &requests::StartRequest,
    connection: &mut ConnectionWrapper,
    process_factory: &Arc<ProcessFactory>,
    statemachine_proxy: StateMachineProxy,
) -> Result<Option<StartSessionAudioResponse>> {
    if !request.audio {
        warn!("Audio disabled");
        return Ok(None);
    }

    let audio_protocol = request
        .kymux_config
        .audio_protocol
        .unwrap_or(kymux::AudioProtocol::Reliable);

    let (endpoint, uri) = connection
        .connection
        .register_audio_endpoint_with_ipc_forward(None, audio_protocol)
        .await?;

    info!("Start Audio avservice. Protocol: {audio_protocol:?}, Endpoint: {endpoint:X}");
    let mut avservice = avservice::Handler::new(avservice::HandlerType::Audio, statemachine_proxy);
    avservice.kymux_start_audio(process_factory, uri).await?;

    Ok(Some(StartSessionAudioResponse {
        avservice,
        endpoint,
    }))
}

async fn start_input(
    request: &requests::StartRequest,
    connection: &mut ConnectionWrapper,
    process_factory: &Arc<ProcessFactory>,
    host_config: inputservice_types::HostConfig,
) -> Result<Option<StartSessionInputResponse>> {
    let Some(inputs_request) = &request.inputs else {
        warn!("Input server disabled");
        return Ok(None);
    };

    let (endpoint, uri) = connection
        .connection
        .register_input_endpoint_with_ipc_forward(None)
        .await?;

    info!("Start Inputservice. Endpoint: {endpoint:X}");

    let service_config = inputservice_types::ServiceConfig {
        cursor_format: inputs_request.cursor_format.clone(),
        gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
    };

    let mut inputservice = inputservice::Handler::new();
    inputservice
        .start_kymux(process_factory, host_config, service_config, uri)
        .await?;

    Ok(Some(StartSessionInputResponse {
        inputservice,
        endpoint,
    }))
}

pub(crate) struct StartSessionVideoResponse {
    display_id: u32,
    avservice: avservice::Handler,
    endpoint: u16,
}

pub(crate) struct StartSessionAudioResponse {
    avservice: avservice::Handler,
    endpoint: u16,
}

pub(crate) struct StartSessionInputResponse {
    inputservice: inputservice::Handler,
    endpoint: u16,
}

pub(crate) struct StartSessionResponse {
    pub(crate) video: Option<Vec<StartSessionVideoResponse>>,
    pub(crate) audio: Option<StartSessionAudioResponse>,
    pub(crate) input: Option<StartSessionInputResponse>,
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

    let connection = backend
        .server
        .wait_connected(Duration::from_millis(500))
        .await?;

    let closed_task = ClosedTask::new(connection.clone(), backend.statemachine_proxy.clone());

    backend.connection = Some(ConnectionWrapper {
        connection,
        closed_task,
    });

    let Some(connection) = backend.connection.as_mut() else {
        warn!("Kymux not connected");
        return Err(Error::KymuxNotConnected);
    };

    // Start services
    let video = start_video(
        &request,
        connection,
        &ctrl.process_factory,
        state_machine_proxy.clone(),
    )
    .await?;

    let audio = start_audio(
        &request,
        connection,
        &ctrl.process_factory,
        state_machine_proxy.clone(),
    )
    .await?;

    let input = start_input(
        &request,
        connection,
        &ctrl.process_factory,
        inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        },
    )
    .await?;

    Ok(StartSessionResponse {
        video,
        audio,
        input,
    })
}
