// Project Kyber: mod.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

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

/// Configuration for starting Kymux infrastructure
pub(crate) struct KymuxInfraConfig {
    pub(crate) listening_port: u16,
    pub(crate) public_port: u16,
    pub(crate) webtransport_gen_certificate: bool,
    pub(crate) cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    pub(crate) private_key: rustls::pki_types::PrivateKeyDer<'static>,
}

/// Running state of the Kymux infrastructure
struct RunningInfra {
    server: KymuxServer,
    kycom: kycom::KyCom,
    certificate_hash: Option<requests::StartKymuxCertificateHash>,
    router: Arc<Mutex<IncomingConnectionRouter>>,
    public_port: u16,
}

/// Shared infrastructure for all Kymux sessions (common server supports both QUIC and WebTransport)
///
/// This object is always alive but can be in a "stopped" or "running" state internally.
/// When stopped, it's an empty shell. When running, it holds the server, kycom, and router.
pub(crate) struct SharedKymuxInfra {
    inner: Option<RunningInfra>,
}

impl SharedKymuxInfra {
    pub(crate) fn new() -> Self {
        Self { inner: None }
    }

    /// Start the infrastructure if not already running. Returns (certificate_hash, public_port).
    pub(crate) async fn start(
        &mut self,
        config: KymuxInfraConfig,
    ) -> Result<(Option<StartKymuxCertificateHash>, u16)> {
        if let Some(ref inner) = self.inner {
            // Already running, return existing config
            info!("Reusing existing shared Kymux common infrastructure");
            return Ok((inner.certificate_hash.clone(), inner.public_port));
        }

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.listening_port);

        info!("Starting common Kymux server on address {addr:?} (supports QUIC + WebTransport)");

        // Choose certificate based on config
        let (cert_chain, private_key, certificate_hash) = if config.webtransport_gen_certificate {
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
            (config.cert_chain, config.private_key, None)
        };

        let server_config = kymux::ServerConfig {
            addr,
            cert_chain,
            private_key,
        };
        let server = kymux::Server::new(server_config).await?;

        // Create the connection router
        let router = Arc::new(Mutex::new(IncomingConnectionRouter::new()));

        // Create the server with the router
        let kymux_server = KymuxServer::new(server, router.clone()).await?;

        // Create shared kycom for all sessions
        const IPC_PORTS: std::ops::Range<u16> = 9091..9100;
        let kycom = kycom::KyCom::start_on_any_port(IPC_PORTS).await?;

        self.inner = Some(RunningInfra {
            server: kymux_server,
            kycom,
            certificate_hash: certificate_hash.clone(),
            router,
            public_port: config.public_port,
        });

        Ok((certificate_hash, config.public_port))
    }

    /// Stop the infrastructure if running. No-op if already stopped.
    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(inner) = self.inner.take() else {
            return Ok(());
        };

        info!("Stopping shared Kymux infrastructure");

        info!("Stopping kycom");
        inner.kycom.stop();

        info!("Stopping Kymux server");
        inner.server.stop().await?;

        Ok(())
    }

    /// Get mutable reference to kycom. Returns error if not running.
    pub(crate) fn kycom_mut(&mut self) -> Result<&mut kycom::KyCom> {
        self.inner
            .as_mut()
            .map(|i| &mut i.kycom)
            .ok_or(Error::KymuxNotListening)
    }

    /// Get reference to router. Returns error if not running.
    pub(crate) fn router(&self) -> Result<&Arc<Mutex<IncomingConnectionRouter>>> {
        self.inner
            .as_ref()
            .map(|i| &i.router)
            .ok_or(Error::KymuxNotListening)
    }

    /// Cleanup expired tokens in the router. No-op if not running.
    pub(crate) async fn cleanup_expired(&self) {
        if let Some(ref inner) = self.inner {
            inner.router.lock().await.cleanup_expired();
        }
    }
}

/// Backend for an established Kymux (QUIC/WebTransport) connection.
///
/// This struct is only created after the client has successfully connected and authenticated.
/// The pending connection state (between start_mux and start_session) is stored in
/// `StreamingSession.pending_kymux_rx` instead.
pub(crate) struct KymuxBackend {
    connection_task: ConnectionTask,
}

impl KymuxBackend {
    fn new(connection_task: ConnectionTask) -> Self {
        Self { connection_task }
    }

    pub(crate) async fn stop(self) -> Result<()> {
        info!("Stop running Kymux Connection");

        let ret = self.connection_task.stop().await;
        if let Err(err) = ret {
            warn!("Kymux Closed task stop raised an error: {err:?}");
        }

        // NOTE: We don't stop the shared kycom or KymuxServer here because other sessions may still be using them
        // They will be stopped when the Controller shuts down

        Ok(())
    }
}

async fn start_kymux(
    ctrl: web::Data<Mutex<Controller>>,
    session_id: uuid::Uuid,
    public_port: u16,
) -> Result<requests::StartKymuxResponse> {
    // Generate auth token for this session
    let token = {
        use rand::distr::SampleString;
        const TOKEN_STRING_LEN: usize = 128;

        let mut rng = rand::rng();
        rand::distr::Alphanumeric.sample_string(&mut rng, TOKEN_STRING_LEN)
    };

    let mut ctrl = ctrl.lock().await;

    // Check if data plane or pending connection already exists for this session
    if ctrl.has_data_plane(&session_id)? {
        return Err(Error::DataPlaneAlreadyOpened);
    }

    // Build config for infrastructure
    // If dataplane port has been overridden, use it directly.
    // This will break controllers deployed behind a NAT.
    let (listening_port, public_port) = if let Some(dataplane_port) = ctrl.config.dataplane_port() {
        (dataplane_port, dataplane_port)
    } else {
        (ctrl.config.port(), public_port)
    };

    let infra_config = KymuxInfraConfig {
        listening_port,
        public_port,
        webtransport_gen_certificate: ctrl.config.webtransport_gen_certificate(),
        cert_chain: ctrl.tls_config.cert_chain.clone(),
        private_key: ctrl.tls_config.private_key.clone_key(),
    };

    // Start infrastructure (no-op if already running)
    let (certificate_hash, public_port) = ctrl
        .shared_kymux_infra
        .lock()
        .await
        .start(infra_config)
        .await?;

    // Register session: creates pending_auth + oneshot channel
    let connection_rx = ctrl
        .shared_kymux_infra
        .lock()
        .await
        .router()?
        .lock()
        .await
        .register_session(token.clone(), session_id);

    let client_session = ctrl.get_session(&session_id)?;

    let Some(stream_session) = client_session.stream_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    // Store the connection receiver - start_session will await on it and create the KymuxBackend
    stream_session.pending_kymux_rx = Some(connection_rx);

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
    session_id: uuid::Uuid,
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
        let mut avservice = avservice::Handler::new(
            avservice::HandlerType::Video,
            Some(session_id),
            statemachine_proxy.clone(),
        );
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
    session_id: uuid::Uuid,
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
    let mut avservice = avservice::Handler::new(
        avservice::HandlerType::Audio,
        Some(session_id),
        statemachine_proxy,
    );
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
    session_id: uuid::Uuid,
    request: requests::StartRequest,
) -> Result<StartSessionResponse> {
    let state_machine_proxy = ctrl.state_machine_proxy()?;

    // Extract data from Controller before session access
    let process_factory = ctrl.process_factory.clone();
    let displays = ctrl.displays.convert();

    // Get shared_infra from Controller
    let shared_infra = ctrl.shared_kymux_infra.clone();

    // Get the pending connection receiver (was created in start_mux)
    let connection_rx = {
        let client_session = ctrl.get_session(&session_id)?;
        let Some(streaming_session) = client_session.stream_session.as_mut() else {
            return Err(Error::ControlPlaneNotOpened);
        };
        streaming_session
            .pending_kymux_rx
            .take()
            .ok_or(Error::KymuxNotListening)?
    };

    // Wait for the connection (server sends it when client connects with valid token)
    let connection = match tokio::time::timeout(Duration::from_millis(500), connection_rx).await {
        Ok(Ok(conn)) => conn,
        Ok(Err(_)) => {
            warn!("Connection channel closed for session {}", session_id);
            return Err(Error::ChannelClosedRx);
        }
        Err(_) => {
            warn!(
                "Timeout waiting for Kymux connection for session {}",
                session_id
            );
            return Err(Error::KymuxAcceptTimeout);
        }
    };

    info!("Connection received for session {}", session_id);

    // Start services
    let (video, audio, input) = {
        let mut infra = shared_infra.lock().await;
        let kycom = infra.kycom_mut()?;

        let video = start_video(
            &request,
            kycom,
            &connection,
            &process_factory,
            session_id,
            state_machine_proxy.clone(),
        )
        .await?;

        let audio = start_audio(
            &request,
            kycom,
            &connection,
            &process_factory,
            session_id,
            state_machine_proxy.clone(),
        )
        .await?;

        let input = start_input(
            &request,
            kycom,
            &connection,
            &process_factory,
            inputservice_types::HostConfig {
                display_list: displays,
            },
        )
        .await?;

        (video, audio, input)
    };

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

    // Create KymuxBackend now that connection is established
    {
        let client_session = ctrl.get_session(&session_id)?;
        let Some(streaming_session) = client_session.stream_session.as_mut() else {
            return Err(Error::ControlPlaneNotOpened);
        };

        let connection_task =
            ConnectionTask::new(connection.clone(), state_machine_proxy, session_id);
        let backend = KymuxBackend::new(connection_task);
        streaming_session.data_plane = Some(DataPlaneBackend::Kymux(backend));
    }

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
