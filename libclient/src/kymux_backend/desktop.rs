use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use log::{error, info, warn};
use tokio::sync::oneshot;
use tokio::task;

use kymux::kyproto;

use super::metrics;
use super::{KymuxBackendConfig, KymuxTransportBackend};
use crate::enums::Message;
use crate::kymux_backend::host_service;
use crate::kymux_backend::host_service::{
    StartSessionAudioResponse, StartSessionResponse, StartSessionVideoResponse,
};
use crate::metrics::MetricsProducer;
use crate::platform::player;
use crate::platform::{InputClient, InputPipeline};
use crate::runtime::Duration;
use crate::{BackendDriver, ConnectionParams, DataPlaneCloseReason, MessageSender};
use crate::{Error, Result};

struct Player(player::Player);

impl Player {
    async fn stop(mut self) -> Result<()> {
        info!("Stop player");
        self.0.stop_async().await?;
        info!("Player stopped");

        Ok(())
    }
}

struct KymuxClosedTask {
    stop_tx: oneshot::Sender<()>,
}

impl KymuxClosedTask {
    fn new(connection: Arc<kyproto::Connection>, msg_sender: MessageSender) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {}
                ret = connection.closed() => {
                    let reason = match ret {
                        Ok(()) => DataPlaneCloseReason::Closed,
                        Err(_) => DataPlaneCloseReason::Error,
                    };

                    let ret = msg_sender.send_async(Message::DataPlaneClosed{ reason }).await;
                    if let Err(err) = ret {
                        warn!("Fail to send DataPlaneClosed: {err:?}");
                    }
                }
            }
        });

        Self { stop_tx }
    }

    fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("ClosedTask already stopped");
        }

        Ok(())
    }
}

async fn connect_quinn(
    conn_params: &ConnectionParams,
    port: u16,
    auth: &kyproto::ClientAuth,
    certificate_hash: Option<String>,
) -> Result<kyproto::Connection> {
    let roots = if let Some(tls_config) = &conn_params.tls_config {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(tls_config.root_certificate.clone().into())
            .map_err(|err| {
                warn!("Failed to crate RootCertStore: {err:?}");
                Error::Rejected
            })?;

        Some(roots)
    } else {
        None
    };

    let server_name = if let Some(tls_host) = &conn_params.tls_host {
        tls_host.clone()
    } else {
        conn_params.host.clone()
    };

    Ok(kymux::connect_with_auth(
        kymux::ClientConfig::Quic {
            addr: SocketAddr::new(conn_params.addr.ip(), port),
            roots,
            server_name,
            certificate_hash,
        },
        auth,
    )
    .await?)
}

fn create_video_config(
    kymux: &mut kymux::ipc::IPCForwardableConnection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &Option<Vec<StartSessionVideoResponse>>,
) -> Result<Option<Vec<player::VideoConfig>>> {
    let video_config = &backend_config.streaming_config.video;
    if video_config.is_empty() {
        return Ok(None);
    }

    let Some(host_config) = host_config else {
        return Ok(None);
    };

    let endpoint_list: HashMap<u32, u16> = host_config
        .iter()
        .map(|video| (video.display_id, video.endpoint))
        .collect();

    let video_protocol = backend_config
        .kymux_config
        .video_protocol
        .unwrap_or(kyproto::VideoProtocol::Reliable);

    let mut player_config_list = vec![];

    for config in video_config {
        let endpoint = endpoint_list.get(&config.display_id).ok_or_else(|| {
            warn!("No endpoint found for display {}", config.display_id);
            Error::Rejected
        })?;

        let uri = kymux.connect_and_forward_video_endpoint(*endpoint, video_protocol)?;

        let player_config = player::VideoConfig {
            source_id: config.display_id,
            software_decode: config.player.software_decode,
            window_handle: config.player.window_handle,
            uri,
            video_buffer: config.player.video_buffer,
        };

        player_config_list.push(player_config);
    }

    Ok(Some(player_config_list))
}

fn create_audio_config(
    kymux: &mut kymux::ipc::IPCForwardableConnection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &Option<StartSessionAudioResponse>,
) -> Result<Option<player::AudioConfig>> {
    let (audio_config, host_config) = match (&backend_config.streaming_config.audio, host_config) {
        (Some(audio_config), Some(host_config)) => (audio_config, host_config),
        (None, None) => return Ok(None),
        _ => {
            error!("Invalid Audio config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    let audio_protocol = backend_config
        .kymux_config
        .audio_protocol
        .unwrap_or(kyproto::AudioProtocol::Reliable);
    let uri = kymux.connect_and_forward_audio_endpoint(host_config.endpoint, audio_protocol)?;
    let audio_buffer = audio_config.audio_buffer();

    Ok(Some(player::AudioConfig { uri, audio_buffer }))
}

async fn start_metrics(
    connection: &kyproto::Connection,
    host_config: &StartSessionResponse,
    metrics: MetricsProducer,
) -> Result<()> {
    let metrics2 = metrics.clone();
    let metrics3 = metrics.clone();

    metrics::start_connection_stats_poller(connection, metrics.clone(), Duration::from_secs(5))
        .await;

    let clock_sync_id = host_config.clock_sync.as_ref().unwrap().endpoint;
    metrics::start_pinger(connection, clock_sync_id, metrics2, Duration::from_secs(10)).await?;

    let remote_network_stats_id = host_config.metrics.as_ref().unwrap().endpoint;
    metrics::start_remote_metrics_receiver(connection, remote_network_stats_id, metrics3).await?;

    if let Some(video) = &host_config.video {
        let endpoints = video
            .iter()
            .filter_map(|v| v.metrics_endpoint.map(|endpoint| (v.display_id, endpoint)));
        for (display_id, endpoint) in endpoints {
            metrics::start_remote_video_metrics_receiver(
                connection,
                display_id,
                endpoint,
                metrics.clone(),
            )
            .await?;
        }
    }

    Ok(())
}

async fn create_player(
    kymux: &mut kymux::ipc::IPCForwardableConnection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
    metrics: bool,
) -> Result<Option<Player>> {
    let video_config = create_video_config(kymux, backend_config, &host_config.video)?;

    let audio_config = create_audio_config(kymux, backend_config, &host_config.audio)?;

    let player = if video_config.is_some() || audio_config.is_some() {
        let audio_buffer = audio_config.as_ref().map(|config| config.audio_buffer);
        let video_buffer = if let Some(video_config) = video_config.as_ref() {
            // XXX VLC only supports one global video_buffer value, but each
            // VideoPlayerConfig has a video_buffer value
            if video_config.is_empty() {
                None
            } else {
                Some(video_config[0].video_buffer)
            }
        } else {
            None
        };

        let mut player = player::Player::new(
            backend_config.msg_sender.clone(),
            video_buffer,
            audio_buffer,
            metrics,
        )?;
        player.play(video_config, audio_config)?;

        Some(Player(player))
    } else {
        None
    };

    Ok(player)
}

async fn start_input(
    kymux: &mut kymux::ipc::IPCForwardableConnection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
    msg_sender: MessageSender,
) -> Result<Option<InputService>> {
    let (input_config, host_config) = match (
        &backend_config.streaming_config.input,
        &host_config.input,
    ) {
        (Some(input_config), Some(host_config)) => (input_config, host_config),
        (None, None) => return Ok(None),
        _ => {
            error!("Invalid Input config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    let input_uri = kymux.connect_and_forward_input_endpoint(host_config.endpoint)?;

    info!("Start input. URI: {input_uri}");

    let create_stream_cb =
        |target, msg_sink| libkynput_rs::kymux::new_stream(target, msg_sink, &input_uri);

    let client = InputClient::new(
        msg_sender,
        input_config.pipeline.get_input_sink(),
        create_stream_cb,
    )?;

    Ok(Some(InputService {
        client,
        pipeline: input_config.pipeline.clone(),
    }))
}

struct InputService {
    pipeline: InputPipeline,
    client: InputClient,
}

async fn start_kymux(backend_config: &KymuxBackendConfig<'_>) -> Result<kyproto::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(
        backend_config.http_client,
        backend_config.kymux_config.backend,
        backend_config.conn_params.port,
    )
    .await?;

    // Extract certificate hash if provided (for self-signed WebTransport certificates)
    let certificate_hash = start_response.certificate_hash.map(|h| h.hash);

    // Start Kymux connection
    let ret = match backend_config.kymux_config.backend {
        KymuxTransportBackend::Quic => {
            info!(
                "Connect using Quic backend. Port: {port}",
                port = start_response.port
            );

            let auth = kymux::kyproto::ClientAuth::new(&start_response.token).map_err(|_| {
                warn!("Got an invalid kymux token from controller");
                Error::KymuxInvalidToken
            })?;
            connect_quinn(
                backend_config.conn_params,
                start_response.port,
                &auth,
                certificate_hash,
            )
            .await
        }
    };

    ret.map_err(|err| {
        warn!("Failed to connect to DataPlane: {err:?}");
        Error::DataPlaneConnectFailed
    })
}

pub(super) struct KymuxBackend {
    kymux: Option<kymux::ipc::IPCForwardableConnection>,
    player: Option<Player>,
    input_service: Option<InputService>,
    closed_task: Option<KymuxClosedTask>,
}

impl KymuxBackend {
    pub(super) async fn new(
        backend_config: KymuxBackendConfig<'_>,
        metrics: Option<MetricsProducer>,
    ) -> Result<Self> {
        let connection = start_kymux(&backend_config).await?;
        let connection = Arc::new(connection);

        let has_metrics = metrics.is_some();

        // Start stream
        let host_config = host_service::start_stream(&backend_config, has_metrics).await?;

        if let Some(metrics) = metrics {
            start_metrics(&connection, &host_config, metrics).await?;
        }

        const IPC_PORTS: std::ops::Range<u16> = 9090..9100;
        let mut kymux =
            kymux::ipc::IPCForwardableConnection::new(connection.clone(), IPC_PORTS).await?;

        // Play Video and Audio
        let player = create_player(&mut kymux, &backend_config, &host_config, has_metrics).await?;

        // Start input
        let input_service = start_input(
            &mut kymux,
            &backend_config,
            &host_config,
            backend_config.msg_sender.clone(),
        )
        .await?;

        // Monitor connection closed
        let closed_task =
            KymuxClosedTask::new(connection.clone(), backend_config.msg_sender.clone());

        Ok(Self {
            kymux: Some(kymux),
            player,
            input_service,
            closed_task: Some(closed_task),
        })
    }
}

#[async_trait]
impl BackendDriver for KymuxBackend {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let player = self.player.as_ref().ok_or(Error::NoAudioPlayer)?;

        player.0.audio_set_volume(volume)?;

        Ok(())
    }

    fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let player = self.player.as_ref().ok_or(Error::NoAudioPlayer)?;

        player.0.audio_set_mute(mute)?;

        Ok(())
    }

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
        if let Some(closed_task) = self.closed_task.take() {
            let ret = closed_task.stop();
            if let Err(err) = ret {
                warn!("Failed to stop KymuxClosedTask: {err:?}");
            }
        }

        if let Some(player) = self.player.take() {
            player.stop().await?;
        }

        if let Some(input_service) = self.input_service.take() {
            input_service.pipeline.stop()?;
        }

        if let Some(kymux) = self.kymux.take() {
            kymux.stop();
        }

        Ok(())
    }
}
