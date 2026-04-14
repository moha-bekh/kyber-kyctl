// Project Kyber: mod.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use bytes::Bytes;
use kymux::{kycom, kynet, kyproto, types};
use log::{info, warn};

use libkypc::process::ProcessFactory;

pub(crate) use server::{IncomingConnectionRouter, KymuxServer};
use tokio::sync::Mutex;

use crate::config::{ListenMode, SessionMode};
use crate::error::{Error, Result};
use crate::inputservice;
use crate::utils::{Task, TypeConvert};
use crate::{Controller, DataPlaneBackend, StateMachineEvent, StateMachineProxy};

use metrics::NetworkStats;
use requests::StartKymuxCertificateHash;

const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(5);
// Must be greater than KEEP_ALIVE_INTERVAL, otherwise healthy connections
// get killed between pings.
const MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

fn run_connection_task(
    connection: Arc<kyproto::Connection>,
    statemachine_proxy: StateMachineProxy,
    session_id: uuid::Uuid,
) -> Task {
    Task::spawn(async move {
        let ret = connection.closed().await;
        info!("Kymux connection closed for session {session_id}: {ret:?}");
        statemachine_proxy
            .send(StateMachineEvent::KymuxTimeout { session_id })
            .await;
    })
}

/// Configuration for starting Kymux infrastructure
pub(crate) struct KymuxInfraConfig {
    pub(crate) listen_mode: ListenMode,
    pub(crate) listening_port: u16,
    pub(crate) webtransport_gen_certificate: bool,
    pub(crate) cert_chain: Vec<rustls::pki_types::CertificateDer<'static>>,
    pub(crate) private_key: rustls::pki_types::PrivateKeyDer<'static>,
}

/// Running state of the Kymux infrastructure
struct RunningInfra {
    server: KymuxServer,
    certificate_hash: Option<requests::StartKymuxCertificateHash>,
    router: Arc<std::sync::Mutex<IncomingConnectionRouter>>,
}

/// Shared infrastructure for all Kymux sessions (common server supports both QUIC and WebTransport)
///
/// This object is always alive but can be in a "stopped" or "running" state internally.
/// When stopped, it's an empty shell. When running, it holds the server and router.
/// kycom lives on the Controller (not here) since it's shared across all data-plane
/// backends (Kymux, WS, RTP), not only Kymux sessions.
pub(crate) struct SharedKymuxInfra {
    inner: Option<RunningInfra>,
}

impl SharedKymuxInfra {
    pub(crate) fn new() -> Self {
        Self { inner: None }
    }

    /// Start the infrastructure if not already running. Returns the certificate_hash.
    pub(crate) async fn start(
        &mut self,
        config: KymuxInfraConfig,
    ) -> Result<Option<StartKymuxCertificateHash>> {
        if let Some(ref inner) = self.inner {
            // Already running, return existing config
            info!("Reusing existing shared Kymux common infrastructure");
            return Ok(inner.certificate_hash.clone());
        }

        let ip_addr = match config.listen_mode {
            ListenMode::Ipv4 => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            // Enable dual-stack: accept both IPv4 and IPv6 on this socket.
            // On Linux this is the default; on Windows it must be set explicitly.
            ListenMode::DualStack => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };

        let addr = SocketAddr::new(ip_addr, config.listening_port);

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

        let options = kynet::common::CommonServerOptions {
            keep_alive_interval: Some(KEEP_ALIVE_INTERVAL),
            max_idle_timeout: Some(MAX_IDLE_TIMEOUT),
        };

        let server = kynet::Connection::start_server_on_addr(
            addr,
            cert_chain.clone(),
            private_key.clone_key(),
            &options,
        )?;

        let server: Box<dyn kynet::Server> = Box::new(server);

        // Create the connection router
        let router = Arc::new(std::sync::Mutex::new(IncomingConnectionRouter::new()));

        // Create the server with the router
        let kymux_server = KymuxServer::new(server, router.clone()).await?;

        self.inner = Some(RunningInfra {
            server: kymux_server,
            certificate_hash: certificate_hash.clone(),
            router,
        });

        Ok(certificate_hash)
    }

    /// Stop the infrastructure if running. No-op if already stopped.
    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(inner) = self.inner.take() else {
            return Ok(());
        };

        info!("Stopping shared Kymux infrastructure");

        info!("Stopping Kymux server");
        inner.server.stop().await?;

        Ok(())
    }

    /// Get reference to router. Returns error if not running.
    pub(crate) fn router(&self) -> Result<&Arc<std::sync::Mutex<IncomingConnectionRouter>>> {
        self.inner
            .as_ref()
            .map(|i| &i.router)
            .ok_or(Error::KymuxNotListening)
    }

    /// Cleanup expired tokens in the router. No-op if not running.
    pub(crate) async fn cleanup_expired(&self) {
        if let Some(ref inner) = self.inner {
            inner.router.lock().unwrap().cleanup_expired();
        }
    }
}

/// Backend for an established Kymux (QUIC/WebTransport) connection.
///
/// This struct is only created after the client has successfully connected and authenticated.
/// The pending connection state (between start_mux and start_session) is stored in
/// `StreamingSession.pending_kymux_rx` instead.
pub(crate) struct KymuxBackend {
    _connection_task: Task,
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
        listen_mode: ctrl.config.listen_mode(),
        listening_port,
        webtransport_gen_certificate: ctrl.config.webtransport_gen_certificate(),
        cert_chain: ctrl.tls_config.cert_chain.clone(),
        private_key: ctrl.tls_config.private_key.clone_key(),
    };

    // Start infrastructure (no-op if already running)
    let certificate_hash = ctrl
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
        .unwrap()
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

/// Start one video stream per `request.video` entry. Mode-agnostic at the
/// top level: the only difference between [`SessionMode::Multi`] and
/// [`SessionMode::Mono`] is the `VideoTopology` we hand to `av_services_manager`
/// (which determines whether kydup is in the path). Everything else
/// (metrics wiring, IDR force, subscription teardown) uses the same code
/// in both modes.
#[allow(clippy::too_many_arguments)]
async fn start_video(
    request: &requests::StartRequest,
    kycom: &mut kymux::kycom::KyCom,
    connection: &kyproto::Connection,
    process_factory: &Arc<ProcessFactory>,
    session_id: uuid::Uuid,
    statemachine_proxy: StateMachineProxy,
    av_services_manager: &mut crate::avservices_manager::AVServicesManager,
    mode: SessionMode,
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

        let client_endpoint = connection.register_video_endpoint(video_protocol).await?;
        let endpoint_id = client_endpoint.id();
        let display_id = stream_request.display_id;

        let config = crate::avservices_manager::VideoStreamConfig {
            bitrate: request.bitrate,
            codec: stream_request.codec.clone(),
            yuv444: stream_request.yuv444,
            intra_refresh: stream_request.intra_refresh,
            video_stream_count,
        };

        // Build the topology. In Mono mode we consume `client_endpoint`
        // here to register it with kycom and use the resulting URI as the
        // avservice's destination; no kydup is created. In Multi mode we
        // keep `client_endpoint` aside to attach it to the kydup that
        // `av_services_manager` creates (or reuses).
        let (acquisition, output) = match mode {
            SessionMode::Multi => {
                let topology = crate::avservices_manager::VideoTopology::Shared { display_id };
                let acq = av_services_manager
                    .acquire_video(
                        kycom,
                        topology,
                        config,
                        request.metrics,
                        process_factory,
                        statemachine_proxy.clone(),
                    )
                    .await?;
                let dup = acq
                    .dup
                    .as_ref()
                    .expect("Shared topology guarantees Some(dup)")
                    .clone();
                // Subscribe + forward, forcing a fresh IDR right after the
                // (post-ready) subscribe so this joining client — which in
                // keyframe-sync mode drops every packet until it sees a keyframe
                // — gets a sync point it cannot miss. Under intra-refresh (the
                // default) the encoder emits no periodic keyframes at all, so
                // this forced IDR is the only reliable sync point, not merely a
                // way to skip the wait for the next one. Forcing it after the
                // subscribe is also why the
                // subscribe is deferred until the endpoint is ready: an idle,
                // not-yet-ready receiver would otherwise gate the whole shared
                // stream during its handshake. The IDR is load-bearing even for
                // a fresh encoder (the very first subscriber): the lazy subscribe
                // happens after the handshake, so any sync frame the encoder
                // emitted at startup was broadcast to zero receivers and lost
                // for this output. Targeted at this exact stream so other
                // streams on the same display are untouched.
                let out = dup.register_and_forward(
                    client_endpoint,
                    av_services_manager.idr_requester(acq.subscription.key().clone()),
                );
                (acq, Some(out))
            }
            SessionMode::Mono => {
                let kycom_uri = kycom.register_and_forward(client_endpoint)?.url();
                let topology = crate::avservices_manager::VideoTopology::Exclusive {
                    session_id,
                    display_id,
                    kycom_uri,
                };
                let acq = av_services_manager
                    .acquire_video(
                        kycom,
                        topology,
                        config,
                        request.metrics,
                        process_factory,
                        statemachine_proxy.clone(),
                    )
                    .await?;
                debug_assert!(acq.dup.is_none());
                (acq, None)
            }
        };

        // Metrics wiring is mode-agnostic. The forwarder lives inside
        // `av_services_manager` either way: in `Shared` it's claim-once across N
        // clients; in `Exclusive` there's just the single client, so the
        // claim is trivial. Either way the kyproto metrics endpoint is
        // attached identically.
        let crate::avservices_manager::VideoAcquisition {
            metrics_claim,
            subscription,
            ..
        } = acquisition;
        let (metrics_endpoint_id, metrics_output) = match (request.metrics, metrics_claim) {
            (true, Some(claim)) => {
                let metrics_endpoint = connection.register_metrics_endpoint().await?;
                let meid = metrics_endpoint.id();
                // Spawn a task that awaits the kyproto ready handshake then
                // hands the `ProtocolSend` to the metrics forwarder via the
                // one-shot upgrade channel. Wrapped in OutputHandle so it's
                // aborted with the rest of the per-session outputs.
                let task = Task::spawn(async move {
                    let protocol = match tokio::time::timeout(
                        Duration::from_secs(5),
                        metrics_endpoint.ready(),
                    )
                    .await
                    {
                        Ok(Ok(p)) => p,
                        Ok(Err(e)) => {
                            warn!("Metrics endpoint ready failed: {e}");
                            return;
                        }
                        Err(_) => {
                            warn!("Metrics endpoint ready timed out");
                            return;
                        }
                    };
                    claim.claim(protocol.send);
                });
                (Some(meid), Some(task))
            }
            (true, None) => {
                // Only reachable for `Shared` topology when the stream was
                // already created without metrics by an earlier client, or
                // when the one-shot slot is already claimed. `Exclusive`
                // entries always have a fresh slot for the single client.
                warn!(
                    "Client requested metrics for display {display_id} but the shared stream's \
                     metrics slot is unavailable (created without metrics, or already claimed); \
                     ignoring",
                );
                (None, None)
            }
            (false, _) => (None, None),
        };

        // Note: the joining keyframe is forced from the kydup post-subscribe
        // hook above (Multi mode), so it lands only once this client is ready
        // and listening. Mono needs no explicit IDR here: the encoder is fresh
        // and writes straight to this single client from its first frame (no
        // kydup, no late join), so the client receives the encoder's own
        // start-of-stream sequence and never has to catch up to a stream
        // already in progress.

        video_streams.push(StartSessionVideoResponse {
            display_id,
            endpoint: endpoint_id,
            metrics_endpoint: metrics_endpoint_id,
            output,
            metrics_output,
            subscription,
        });
    }

    Ok(Some(video_streams))
}

/// Audio counterpart of [`start_video`]. Same mode-agnostic structure.
#[allow(clippy::too_many_arguments)]
async fn start_audio(
    request: &requests::StartRequest,
    kycom: &mut kymux::kycom::KyCom,
    connection: &kyproto::Connection,
    process_factory: &Arc<ProcessFactory>,
    session_id: uuid::Uuid,
    statemachine_proxy: StateMachineProxy,
    av_services_manager: &mut crate::avservices_manager::AVServicesManager,
    mode: SessionMode,
) -> Result<Option<StartSessionAudioResponse>> {
    if !request.audio {
        warn!("Audio disabled");
        return Ok(None);
    }

    let audio_protocol = request
        .kymux_config
        .audio_protocol
        .unwrap_or(kyproto::AudioProtocol::Reliable);

    let client_endpoint = connection.register_audio_endpoint(audio_protocol).await?;
    let endpoint_id = client_endpoint.id();

    let (acquisition, output) = match mode {
        SessionMode::Multi => {
            let topology = crate::avservices_manager::AudioTopology::Shared;
            let acq = av_services_manager
                .acquire_audio(kycom, topology, process_factory, statemachine_proxy)
                .await?;
            let dup = acq
                .dup
                .as_ref()
                .expect("Shared topology guarantees Some(dup)")
                .clone();
            let out = dup.register_and_forward(client_endpoint);
            (acq, Some(out))
        }
        SessionMode::Mono => {
            let kycom_uri = kycom.register_and_forward(client_endpoint)?.url();
            let topology = crate::avservices_manager::AudioTopology::Exclusive {
                session_id,
                kycom_uri,
            };
            let acq = av_services_manager
                .acquire_audio(kycom, topology, process_factory, statemachine_proxy)
                .await?;
            debug_assert!(acq.dup.is_none());
            (acq, None)
        }
    };

    Ok(Some(StartSessionAudioResponse {
        endpoint: endpoint_id,
        output,
        subscription: acquisition.subscription,
    }))
}

async fn start_input(
    request: &requests::StartRequest,
    kycom: &mut kycom::KyCom,
    connection: &kyproto::Connection,
    process_factory: &Arc<ProcessFactory>,
    host_config: kynputservice_types::HostConfig,
) -> Result<Option<StartSessionInputResponse>> {
    let Some(inputs_request) = &request.inputs else {
        warn!("Input server disabled");
        return Ok(None);
    };

    let endpoint = connection.register_input_endpoint().await?;

    let endpoint_id = endpoint.id();
    let uri = kycom.register_and_forward(endpoint)?.url();

    info!("Start Inputservice. Endpoint: {endpoint_id:X}");

    let service_config = kynputservice_types::ServiceConfig {
        cursor_format: inputs_request.cursor_format.clone(),
        gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
        clipboard: inputs_request.clipboard,
    };

    let (inputservice, event) =
        inputservice::Handler::start_kymux(process_factory, host_config, service_config, uri)
            .await?;

    let clipboard = match event {
        Some(kynputservice_types::Event::Connected { clipboard }) => clipboard,
        _ => false,
    };

    Ok(Some(StartSessionInputResponse {
        inputservice,
        endpoint: endpoint_id,
        clipboard,
    }))
}

pub(crate) struct StartSessionVideoResponse {
    pub(crate) display_id: u32,
    pub(crate) endpoint: u16,
    pub(crate) metrics_endpoint: Option<u16>,
    /// `Some` when the entry is a `Shared` kydup output (multi mode), `None`
    /// when the avservice writes directly to the client endpoint via kycom
    /// (mono mode).
    pub(crate) output: Option<Task>,
    /// `Some` when the kyproto metrics endpoint task was spawned.
    pub(crate) metrics_output: Option<Task>,
    pub(crate) subscription: crate::avservices_manager::VideoSubscription,
}

pub(crate) struct StartSessionAudioResponse {
    pub(crate) endpoint: u16,
    /// See [`StartSessionVideoResponse::output`].
    pub(crate) output: Option<Task>,
    pub(crate) subscription: crate::avservices_manager::AudioSubscription,
}

pub(crate) struct StartSessionInputResponse {
    inputservice: inputservice::Handler,
    endpoint: u16,
    clipboard: bool,
}

pub(crate) struct StartSessionClockSyncResponse {
    task: Task,
    endpoint: u16,
}

pub(crate) struct StartSessionMetricsResponse {
    task: Task,
    endpoint: u16,
}

pub(crate) struct StartSessionResponse {
    pub(crate) video: Option<Vec<StartSessionVideoResponse>>,
    pub(crate) audio: Option<StartSessionAudioResponse>,
    pub(crate) input: Option<StartSessionInputResponse>,
    pub(crate) clock_sync: Option<StartSessionClockSyncResponse>,
    pub(crate) metrics: Option<StartSessionMetricsResponse>,
}

fn run_clock_sync_task(endpoint: kyproto::ClockSyncServerEndpoint) -> Task {
    Task::spawn(async move {
        let mut protocol = endpoint.ready().await.unwrap();
        protocol.serve().await.unwrap();
    })
}

fn run_metrics_task(
    endpoint: types::MetricsServerEndpoint,
    stats_provider: kyproto::KyProtoStatsProvider,
) -> Task {
    Task::spawn(async move {
        let _ = run_stats_poller(endpoint, stats_provider, Duration::from_secs(5)).await;
    })
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

    // Start services (using shared kycom and av_services_manager from Controller)
    ctrl.ensure_kycom().await?;

    let mode = ctrl.config.session_mode();

    // Disjoint borrows: access kycom and av_services_manager independently. kycom is
    // `&mut` everywhere; kydup's upstream endpoints are plugged via
    // `kycom.register_and_forward(...)`, same as the per-client video / audio
    // endpoints in mono mode.
    let kycom = ctrl.kycom.as_mut().expect("ensure_kycom guarantees Some");
    let av_services_manager = &mut ctrl.av_services_manager;

    let video = start_video(
        &request,
        kycom,
        &connection,
        &process_factory,
        session_id,
        state_machine_proxy.clone(),
        av_services_manager,
        mode,
    )
    .await?;

    let audio = start_audio(
        &request,
        kycom,
        &connection,
        &process_factory,
        session_id,
        state_machine_proxy.clone(),
        av_services_manager,
        mode,
    )
    .await?;

    // Input still uses register_and_forward (1:1, not shared via kydup)
    let input = start_input(
        &request,
        kycom,
        &connection,
        &process_factory,
        kynputservice_types::HostConfig {
            display_list: displays,
        },
    )
    .await?;

    let clock_sync = if request.metrics {
        let endpoint: kyproto::ClockSyncServerEndpoint =
            connection.register_clock_sync_endpoint().await?;
        let endpoint_id = endpoint.id();
        info!("Start clock sync. Endpoint: {endpoint_id:X}");

        let task = run_clock_sync_task(endpoint);
        Some(StartSessionClockSyncResponse {
            endpoint: endpoint_id,
            task,
        })
    } else {
        None
    };

    let metrics = if request.metrics {
        let endpoint = connection.register_metrics_endpoint().await?;
        let endpoint_id = endpoint.id();
        info!("Start metrics server. Endpoint: {endpoint_id:X}");

        let stats_provider = connection.stats_provider();
        let task = run_metrics_task(endpoint, stats_provider);

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

        let _connection_task =
            run_connection_task(connection.clone(), state_machine_proxy, session_id);
        let backend = KymuxBackend { _connection_task };
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
    endpoint: types::MetricsServerEndpoint,
    stats_provider: kyproto::KyProtoStatsProvider,
    interval: Duration,
) -> Result<()> {
    let mut protocol = endpoint.ready().await?;
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
        let packet = types::MetricsPacket { payload };
        protocol.send.send(packet).await?;
    }
}
