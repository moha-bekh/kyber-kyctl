mod metrics;
pub(crate) mod requests;
mod server;
mod webtransport;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use bytes::Bytes;
use kymux::{
    ClockSyncServerEndpoint, KyProtoStatsProvider, MetricsPacket, MetricsServerEndpoint,
    ProtocolEndpoint,
};
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

use metrics::NetworkStats;
use requests::{KymuxTransportBackend, StartKymuxCertificateHash};

struct ConnectionTask {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,
}

impl ConnectionTask {
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

pub(crate) struct KymuxBackend {
    pub(crate) server: KymuxServer,
    connection_task: Option<ConnectionTask>,
    services_sink: kymux::ipc::IpcHandler,
    statemachine_proxy: StateMachineProxy,
}

impl KymuxBackend {
    async fn new(server: KymuxServer, statemachine_proxy: StateMachineProxy) -> Result<Self> {
        const IPC_PORT: u16 = 9091;
        let services_sink = kymux::ipc::IpcHandler::new(IPC_PORT).await?;

        Ok(Self {
            server,
            connection_task: None,
            services_sink,
            statemachine_proxy,
        })
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        if let Some(task) = self.connection_task.take() {
            info!("Stop running Kymux Connection");

            let ret = task.stop().await;
            if let Err(err) = ret {
                warn!("Kymux Closed task stop raised an error: {err:?}");
            }
        };

        info!("Stop running IPC Services sink");
        let ret = self.services_sink.stop().await;
        if let Err(err) = ret {
            warn!("Kymux IPC stop raised an error: {err:?}");
        }

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
    let (listening_port, public_port, tls_config, webtransport_gen_certificate) = {
        let ctrl = ctrl.lock().await;

        // If dataplane port has been overridden, use it directly.
        // This will break controllers deployed behind a NAT.
        let (listening_port, public_port) =
            if let Some(dataplane_port) = ctrl.config.dataplane_port() {
                (dataplane_port, dataplane_port)
            } else {
                (ctrl.config.port(), public_port)
            };

        let webtransport_gen_certificate = ctrl.config.webtransport_gen_certificate();

        (
            listening_port,
            public_port,
            ctrl.tls_config.clone(),
            webtransport_gen_certificate,
        )
    };

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listening_port);

    // Start Listen task
    let (server, certificate_hash) = match backend {
        KymuxTransportBackend::Quic => {
            info!("Start Quic listener on address {addr:?}");

            let config = kymux::ServerConfig::Quic {
                addr,
                cert_chain: tls_config.cert_chain.clone(),
                private_key: tls_config.private_key.clone_key(),
            };
            let server = kymux::Server::new(config).await?;
            (server, None)
        }
        KymuxTransportBackend::Webtransport => {
            info!("Start webtransport listener on address {addr:?}");

            let (cert_chain, private_key, certificate_hash) = if webtransport_gen_certificate {
                info!("Generate certificate for webtransport connection");

                let cert = webtransport::gen_webtransport_cert()?;

                let certificate_hash = StartKymuxCertificateHash {
                    algorithm: "sha-256".into(),
                    hash: cert.cert_digest,
                };

                (vec![cert.cert_der], cert.key_der, Some(certificate_hash))
            } else {
                (
                    tls_config.cert_chain.clone(),
                    tls_config.private_key.clone_key(),
                    None,
                )
            };

            let server_config = kymux::ServerConfig::Wtransport {
                addr,
                cert_chain,
                private_key,
            };
            let server = kymux::Server::new(server_config).await?;

            (server, certificate_hash)
        }
    };

    // Start Listen task wrapper
    let server = KymuxServer::new(server).await?;

    {
        let mut ctrl = ctrl.lock().await;
        let Some(client_session) = ctrl.client_session.as_mut() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        let backend = KymuxBackend::new(server, statemachine_proxy).await?;
        client_session.data_plane = Some(DataPlaneBackend::Kymux(backend));
    }

    Ok(requests::StartKymuxResponse {
        port: public_port,
        certificate_hash,
    })
}

async fn start_video(
    request: &requests::StartRequest,
    services_sink: &kymux::ipc::IpcHandler,
    connection: &kymux::Connection,
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

        let endpoint = connection
            .register_video_endpoint(None, video_protocol)
            .await?;

        let endpoint_id = endpoint.id();
        let uri = services_sink.register_and_forward(endpoint)?;

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {endpoint_id:X}");
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
            endpoint: endpoint_id,
        })
    }

    Ok(Some(video_streams))
}

async fn start_audio(
    request: &requests::StartRequest,
    services_sink: &kymux::ipc::IpcHandler,
    connection: &kymux::Connection,
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

    let endpoint = connection
        .register_audio_endpoint(None, audio_protocol)
        .await?;

    let endpoint_id = endpoint.id();
    let uri = services_sink.register_and_forward(endpoint)?;

    info!("Start Audio avservice. Protocol: {audio_protocol:?}, Endpoint: {endpoint_id:X}");
    let mut avservice = avservice::Handler::new(avservice::HandlerType::Audio, statemachine_proxy);
    avservice.kymux_start_audio(process_factory, uri).await?;

    Ok(Some(StartSessionAudioResponse {
        avservice,
        endpoint: endpoint_id,
    }))
}

async fn start_input(
    request: &requests::StartRequest,
    services_sink: &kymux::ipc::IpcHandler,
    connection: &kymux::Connection,
    process_factory: &Arc<ProcessFactory>,
    host_config: inputservice_types::HostConfig,
) -> Result<Option<StartSessionInputResponse>> {
    let Some(inputs_request) = &request.inputs else {
        warn!("Input server disabled");
        return Ok(None);
    };

    let endpoint = connection.register_input_endpoint(None).await?;

    let endpoint_id = endpoint.id();
    let uri = services_sink.register_and_forward(endpoint)?;

    info!("Start Inputservice. Endpoint: {endpoint_id:X}");

    let service_config = inputservice_types::ServiceConfig {
        cursor_format: inputs_request.cursor_format.clone(),
        gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
        clipboard: inputs_request.clipboard,
    };

    let mut inputservice = inputservice::Handler::new();
    let (clipboard,) = match inputservice
        .start_kymux(process_factory, host_config, service_config, uri)
        .await?
    {
        Some(inputservice_types::Event::Connected { clipboard }) => (clipboard,),
        _ => (false,),
    };

    Ok(Some(StartSessionInputResponse {
        inputservice,
        endpoint: endpoint_id,
        clipboard,
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
    clipboard: bool,
}

pub(crate) struct StartSessionClockSyncResponse {
    task: ClockSyncTask,
    endpoint: u16,
}

pub(crate) struct StartSessionMetricsResponse {
    task: MetricsTask,
    endpoint: u16,
}

pub(crate) struct StartSessionResponse {
    pub(crate) video: Option<Vec<StartSessionVideoResponse>>,
    pub(crate) audio: Option<StartSessionAudioResponse>,
    pub(crate) input: Option<StartSessionInputResponse>,
    pub(crate) clock_sync: Option<StartSessionClockSyncResponse>,
    pub(crate) metrics: Option<StartSessionMetricsResponse>,
}

pub(crate) struct ClockSyncTask {
    stop_tx: oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl ClockSyncTask {
    fn new(endpoint: ClockSyncServerEndpoint) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                _ = Self::clock_sync_task(endpoint) => {
                }

            }
        });

        Self { stop_tx, handle }
    }

    pub(crate) async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.handle.await;
    }

    async fn clock_sync_task(endpoint: ClockSyncServerEndpoint) {
        let mut protocol = endpoint.ready().await.unwrap();
        protocol.serve().await.unwrap();
    }
}

pub(crate) struct MetricsTask {
    stop_tx: oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl MetricsTask {
    fn new(endpoint: MetricsServerEndpoint, stats_provider: KyProtoStatsProvider) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                _ = run_stats_poller(endpoint, stats_provider, Duration::from_secs(5)) => {
                }

            }
        });

        Self { stop_tx, handle }
    }

    pub(crate) async fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.handle.await;
    }
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

    // Start services
    let video = start_video(
        &request,
        &backend.services_sink,
        &connection,
        &ctrl.process_factory,
        state_machine_proxy.clone(),
    )
    .await?;

    let audio = start_audio(
        &request,
        &backend.services_sink,
        &connection,
        &ctrl.process_factory,
        state_machine_proxy.clone(),
    )
    .await?;

    let input = start_input(
        &request,
        &backend.services_sink,
        &connection,
        &ctrl.process_factory,
        inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        },
    )
    .await?;

    let clock_sync = {
        let endpoint: ClockSyncServerEndpoint =
            connection.register_clock_sync_endpoint(None).await?;
        let endpoint_id = endpoint.id();
        info!("Start clock sync. Endpoint: {endpoint_id:X}");

        let task = ClockSyncTask::new(endpoint);
        Some(StartSessionClockSyncResponse {
            endpoint: endpoint_id,
            task,
        })
    };

    let metrics = {
        let endpoint = connection.register_metrics_endpoint(None).await?;
        let endpoint_id = endpoint.id();
        info!("Start metrics server. Endpoint: {endpoint_id:X}");

        let stats_provider = connection.stats_provider();
        let task = MetricsTask::new(endpoint, stats_provider);

        Some(StartSessionMetricsResponse {
            endpoint: endpoint_id,
            task,
        })
    };

    backend.connection_task = Some(ConnectionTask::new(
        connection.clone(),
        backend.statemachine_proxy.clone(),
    ));

    Ok(StartSessionResponse {
        video,
        audio,
        input,
        clock_sync,
        metrics,
    })
}

async fn run_stats_poller(
    endpoint: MetricsServerEndpoint,
    stats_provider: KyProtoStatsProvider,
    interval: Duration,
) -> Result<()> {
    let mut protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::Kymux { source: err.into() })?;
    let mut interval = tokio::time::interval(interval);
    loop {
        interval.tick().await;
        let connection_stats = stats_provider.connection_stats().await;
        let network_stats = NetworkStats {
            rtt_micros: connection_stats
                .rtt
                .map(|d| u64::try_from(d.as_micros()).unwrap()),
            packets_lost: connection_stats.packets_lost,
        };
        let payload = Bytes::from(rmp_serde::to_vec(&network_stats).unwrap());
        let packet = MetricsPacket { payload };
        protocol
            .send(packet)
            .await
            .map_err(|err| Error::Kymux { source: err.into() })?;
    }
}
