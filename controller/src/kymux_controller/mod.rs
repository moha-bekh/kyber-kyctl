mod metrics;
pub(crate) mod requests;
mod server;
mod webtransport;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use bytes::Bytes;
use kymux::kycom;
use kymux::kyproto::{self, ProtocolEndpoint};
use log::{info, warn};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{self, JoinHandle};

use libkypc::process::ProcessFactory;

pub(crate) use server::{IncomingConnectionRouter, KymuxServer};

use crate::avservice;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::utils::TypeConvert;
use crate::{Controller, DataPlaneBackend, StateMachineEvent, StateMachineProxy};

use metrics::NetworkStats;
use requests::StartKymuxCertificateHash;

struct ConnectionTask {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,
}

impl ConnectionTask {
    fn new(
        connection: Arc<kyproto::Connection>,
        statemachine_proxy: StateMachineProxy,
        session_id: uuid::Uuid,
    ) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join_handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                ret = Self::run_task(connection, statemachine_proxy, session_id) => {
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
        connection: Arc<kyproto::Connection>,
        statemachine_proxy: StateMachineProxy,
        session_id: uuid::Uuid,
    ) -> Result<()> {
        let ret = connection.closed().await;
        info!("Kymux connection closed for session {session_id}: {ret:?}");

        statemachine_proxy
            .send(StateMachineEvent::KymuxTimeout { session_id })
            .await;

        Ok(())
    }
}

// Shared infrastructure for Kymux (server supports both QUIC and WebTransport on same port)
pub(crate) struct SharedKymuxInfra {
    pub(crate) server: KymuxServer,
    pub(crate) kycom: kycom::KyCom,
    pub(crate) certificate_hash: Option<requests::StartKymuxCertificateHash>,
    pub(crate) router: Arc<Mutex<IncomingConnectionRouter>>,
}

impl SharedKymuxInfra {
    pub(crate) async fn stop(self) -> Result<()> {
        info!("Stopping shared Kymux infrastructure");

        info!("Stopping kycom");
        self.kycom.stop();

        info!("Stopping Kymux server");
        self.server.stop().await?;

        Ok(())
    }
}

pub(crate) struct KymuxBackend {
    /// Receiver for the authenticated QUIC connection.
    ///
    /// The connection is guaranteed to be for this session: IncomingConnectionRouter
    /// validates the auth token and routes the connection through this session-specific channel.
    connection_rx: Option<oneshot::Receiver<Arc<kyproto::Connection>>>,
    connection_task: Option<ConnectionTask>,
    statemachine_proxy: StateMachineProxy,
}

impl KymuxBackend {
    fn new(
        connection_rx: oneshot::Receiver<Arc<kyproto::Connection>>,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        Self {
            connection_rx: Some(connection_rx),
            connection_task: None,
            statemachine_proxy,
        }
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        if let Some(task) = self.connection_task.take() {
            info!("Stop running Kymux Connection");

            let ret = task.stop().await;
            if let Err(err) = ret {
                warn!("Kymux Closed task stop raised an error: {err:?}");
            }
        };

        // NOTE: We don't stop the shared kycom or KymuxServer here
        // They will be stopped when the Controller shuts down via shared_kymux_infra

        Ok(())
    }
}

async fn start_kymux(
    ctrl: web::Data<Mutex<Controller>>,
    public_port: u16,
    statemachine_proxy: StateMachineProxy,
) -> Result<requests::StartKymuxResponse> {
    // Check if data plane is already opened
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

    // Check if shared infrastructure already exists, if not create it
    let certificate_hash = {
        let mut ctrl = ctrl.lock().await;

        if let Some(existing_infra) = &ctrl.shared_kymux_infra {
            // Infrastructure exists, reuse it
            info!("Reusing existing shared Kymux infrastructure");
            existing_infra.lock().await.certificate_hash.clone()
        } else {
            // Create new infrastructure
            info!(
                "Starting Kymux server on address {addr:?} (supports QUIC + WebTransport)"
            );

            // Choose certificate based on config:
            // - Dev mode (webtransport_gen_certificate=true): Generate short-lived cert with hash
            //   Required for WebTransport clients using serverCertificateHashes
            // - Prod mode (webtransport_gen_certificate=false): Use TLS cert from config
            //   Assumes cert is signed by a trusted CA (works for both QUIC and WebTransport)
            let (cert_chain, private_key, certificate_hash) = if webtransport_gen_certificate {
                info!("Generating short-lived certificate for WebTransport compatibility");

                let wt_cert = webtransport::gen_webtransport_cert()?;
                let cert_hash = StartKymuxCertificateHash {
                    algorithm: "sha-256".to_string(),
                    hash: wt_cert.cert_digest.clone(),
                };

                info!("Generated certificate with hash: {}", wt_cert.cert_digest);

                (vec![wt_cert.cert_der], wt_cert.key_der, Some(cert_hash))
            } else {
                info!("Using TLS certificate from config (must be CA-signed for WebTransport)");

                (
                    tls_config.cert_chain.clone(),
                    tls_config.private_key.clone_key(),
                    None,
                )
            };

            let config = kymux::ServerConfig {
                addr,
                cert_chain,
                private_key,
            };
            let server = kymux::Server::new(config).await?;

            // Create the connection router
            let router = Arc::new(Mutex::new(IncomingConnectionRouter::new()));

            // Create the server with the router
            let kymux_server = KymuxServer::new(server, router.clone()).await?;

            // Create shared kycom
            const IPC_PORTS: std::ops::Range<u16> = 9091..9100;
            let kycom = kycom::KyCom::start_on_any_port(IPC_PORTS).await?;

            let infra = SharedKymuxInfra {
                server: kymux_server,
                kycom,
                certificate_hash: certificate_hash.clone(),
                router,
            };
            let shared = Arc::new(tokio::sync::Mutex::new(infra));

            // Store shared infrastructure in Controller
            ctrl.shared_kymux_infra = Some(shared);

            certificate_hash
        }
    };

    // Generate auth token for this session
    let token = {
        use rand::distr::SampleString;
        const TOKEN_STRING_LEN: usize = 128;

        let mut rng = rand::rng();
        rand::distr::Alphanumeric.sample_string(&mut rng, TOKEN_STRING_LEN)
    };

    {
        let mut ctrl = ctrl.lock().await;

        // Get shared_infra first (clones the Arc, releases borrow)
        let shared_infra = ctrl
            .shared_kymux_infra
            .as_ref()
            .ok_or(Error::KymuxNotListening)?
            .clone();

        let Some(client_session) = ctrl.client_session.as_mut() else {
            return Err(Error::SessionNotOpened);
        };

        let session_id = client_session.uid;

        // Register session in router: creates pending_auth + oneshot channel
        let connection_rx = shared_infra
            .lock()
            .await
            .router
            .lock()
            .await
            .register_session(token.clone(), session_id);

        let Some(stream_session) = client_session.stream_session.as_mut() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        // Store the Receiver in KymuxBackend - start_session will await on it
        let backend = KymuxBackend::new(connection_rx, statemachine_proxy);
        stream_session.data_plane = Some(DataPlaneBackend::Kymux(backend));
    }

    Ok(requests::StartKymuxResponse {
        port: public_port,
        certificate_hash,
        token,
    })
}

async fn start_video(
    request: &requests::StartRequest,
    kycom: &mut kycom::KyCom,
    connection: &kyproto::Connection,
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
            .unwrap_or(kyproto::VideoProtocol::Reliable);

        let endpoint = connection
            .register_video_endpoint(None, video_protocol)
            .await?;

        let endpoint_id = endpoint.id();
        let uri = kycom.register_and_forward(endpoint)?.url();

        let (metrics_uri, metrics_endpoint_id) = if request.metrics {
            let metrics_endpoint = connection.register_metrics_endpoint(None).await?;
            let metrics_endpoint_id = metrics_endpoint.id();

            let metrics_uri = kycom.register_and_forward(metrics_endpoint)?.url();

            (Some(metrics_uri), Some(metrics_endpoint_id))
        } else {
            (None, None)
        };

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {endpoint_id:X}");
        let mut avservice =
            avservice::Handler::new(avservice::HandlerType::Video, None, statemachine_proxy.clone());
        avservice
            .kymux_start_video(
                process_factory,
                stream_request.display_id,
                uri,
                metrics_uri,
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
            metrics_endpoint: metrics_endpoint_id,
        })
    }

    Ok(Some(video_streams))
}

async fn start_audio(
    request: &requests::StartRequest,
    kycom: &mut kycom::KyCom,
    connection: &kyproto::Connection,
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
        .unwrap_or(kyproto::AudioProtocol::Reliable);

    let endpoint = connection
        .register_audio_endpoint(None, audio_protocol)
        .await?;

    let endpoint_id = endpoint.id();
    let uri = kycom.register_and_forward(endpoint)?.url();

    info!("Start Audio avservice. Protocol: {audio_protocol:?}, Endpoint: {endpoint_id:X}");
    let mut avservice = avservice::Handler::new(avservice::HandlerType::Audio, None, statemachine_proxy);
    avservice.kymux_start_audio(process_factory, uri).await?;

    Ok(Some(StartSessionAudioResponse {
        avservice,
        endpoint: endpoint_id,
    }))
}

async fn start_input(
    request: &requests::StartRequest,
    kycom: &mut kycom::KyCom,
    connection: &kyproto::Connection,
    process_factory: &Arc<ProcessFactory>,
    host_config: inputservice_types::HostConfig,
) -> Result<Option<StartSessionInputResponse>> {
    let Some(inputs_request) = &request.inputs else {
        warn!("Input server disabled");
        return Ok(None);
    };

    let endpoint = connection.register_input_endpoint(None).await?;

    let endpoint_id = endpoint.id();
    let uri = kycom.register_and_forward(endpoint)?.url();

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
    metrics_endpoint: Option<u16>,
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
    fn new(endpoint: kyproto::ClockSyncServerEndpoint) -> Self {
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

    async fn clock_sync_task(endpoint: kyproto::ClockSyncServerEndpoint) {
        let mut protocol = endpoint.ready().await.unwrap();
        protocol.serve().await.unwrap();
    }
}

pub(crate) struct MetricsTask {
    stop_tx: oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
}

impl MetricsTask {
    fn new(
        endpoint: kyproto::MetricsServerEndpoint,
        stats_provider: kyproto::KyProtoStatsProvider,
    ) -> Self {
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

    // Get session info
    let Some(client_session) = ctrl.client_session.as_mut() else {
        return Err(Error::SessionNotOpened);
    };
    let session_id = client_session.uid;
    let Some(streaming_session) = client_session.stream_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let Some(DataPlaneBackend::Kymux(backend)) = streaming_session.data_plane.as_mut() else {
        return Err(Error::KymuxNotListening);
    };

    // Get shared infrastructure
    let shared_infra = ctrl
        .shared_kymux_infra
        .as_ref()
        .ok_or(Error::KymuxNotListening)?
        .clone();

    // Wait for connection via the oneshot channel
    let connection_rx = backend.connection_rx.take().ok_or(Error::KymuxNotListening)?;

    let connection = match tokio::time::timeout(Duration::from_millis(500), connection_rx).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(_)) => {
            warn!("Connection channel closed");
            return Err(Error::ChannelClosedRx);
        }
        Err(_) => {
            warn!("No Kymux connection received after 500ms");
            return Err(Error::KymuxAcceptTimeout);
        }
    };

    // Get mutable access to kycom from shared infra
    let mut infra = shared_infra.lock().await;

    // Start services
    let video = start_video(
        &request,
        &mut infra.kycom,
        &connection,
        &ctrl.process_factory,
        state_machine_proxy.clone(),
    )
    .await?;

    let audio = start_audio(
        &request,
        &mut infra.kycom,
        &connection,
        &ctrl.process_factory,
        state_machine_proxy.clone(),
    )
    .await?;

    let input = start_input(
        &request,
        &mut infra.kycom,
        &connection,
        &ctrl.process_factory,
        inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        },
    )
    .await?;

    // Release infra lock before setting up connection task
    drop(infra);

    let clock_sync = if request.metrics {
        let endpoint: kyproto::ClockSyncServerEndpoint =
            connection.register_clock_sync_endpoint(None).await?;
        let endpoint_id = endpoint.id();
        info!("Start clock sync. Endpoint: {endpoint_id:X}");

        let task = ClockSyncTask::new(endpoint);
        Some(StartSessionClockSyncResponse {
            endpoint: endpoint_id,
            task,
        })
    } else {
        None
    };

    let metrics = if request.metrics {
        let endpoint = connection.register_metrics_endpoint(None).await?;
        let endpoint_id = endpoint.id();
        info!("Start metrics server. Endpoint: {endpoint_id:X}");

        let stats_provider = connection.stats_provider();
        let task = MetricsTask::new(endpoint, stats_provider);

        Some(StartSessionMetricsResponse {
            endpoint: endpoint_id,
            task,
        })
    } else {
        None
    };

    backend.connection_task = Some(ConnectionTask::new(
        connection.clone(),
        backend.statemachine_proxy.clone(),
        session_id,
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
    endpoint: kyproto::MetricsServerEndpoint,
    stats_provider: kyproto::KyProtoStatsProvider,
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
        let protocol_stats = stats_provider.protocol_stats();
        let network_stats = NetworkStats {
            rtt_micros: connection_stats
                .rtt
                .map(|d| u64::try_from(d.as_micros()).unwrap()),
            packets_lost: connection_stats.packets_lost,
            dropped_packets: protocol_stats.dropped_packets,
        };
        let payload = Bytes::from(rmp_serde::to_vec(&network_stats).unwrap());
        let packet = kyproto::MetricsPacket { payload };
        protocol
            .send(packet)
            .await
            .map_err(|err| Error::Kymux { source: err.into() })?;
    }
}
