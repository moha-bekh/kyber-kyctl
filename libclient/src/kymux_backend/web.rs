use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use log::{error, info, warn};
use tokio::sync::oneshot;
use wasm_bindgen_futures::spawn_local;

use kymux::{
    AVPacket, AudioProtocol, InputPacket, ProtocolEndpoint, ProtocolRecv, ProtocolSend,
    VideoProtocol,
};
use libkynput::bufferproxy::{BufferProxy, BufferProxyReceiver, BufferProxySender};
use libkynput::{InputNetworkStream, InputNetworkStreamMsg, InputNetworkStreamObserver};

use super::{KymuxBackendConfig, KymuxTransportBackend};
use crate::enums::Message;
use crate::kymux_backend::host_service;
use crate::kymux_backend::host_service::{
    StartKymuxCertificateHash, StartSessionAudioResponse, StartSessionResponse,
    StartSessionVideoResponse,
};
use crate::platform::player::{
    AudioPlayer, AudioPlayerHandle, PacketSink, VideoDecoderConfig, VideoPlayer, VideoPlayerHandle,
};
use crate::platform::AudioPlayerConfig;
use crate::platform::InputMsgSink;
use crate::{BackendDriver, DataPlaneCloseReason, MessageProxy, VideoSessionConfig};
use crate::{Error, Result};

#[derive(Clone)]
struct CloseRequestState(Rc<RefCell<bool>>);

impl CloseRequestState {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(false)))
    }

    fn close(&self) {
        let mut value = self.0.borrow_mut();
        *value = true;
    }

    fn is_closed(&self) -> bool {
        let value = self.0.borrow();
        *value
    }
}

struct InputService {
    proxy: Arc<BufferProxy>,
    pipeline: Arc<libkynput::InputPipeline>,
}

struct Player {
    video: Vec<VideoPlayerHandle>,
    audio: Option<AudioPlayerHandle>,
}

impl Player {
    async fn stop(self) -> Result<()> {
        for mut video_player in self.video {
            info!("Stop video player");
            let ret = video_player.stop().await;
            if let Err(err) = ret {
                warn!("Video player stop failed: {err:?}");
            }
        }

        if let Some(mut audio_player) = self.audio {
            info!("Stop audio player");
            let ret = audio_player.stop().await;
            if let Err(err) = ret {
                warn!("Audio player stop failed: {err:?}");
            }
        }

        Ok(())
    }
}

struct KymuxClosedTask {
    stop_tx: oneshot::Sender<()>,
}

impl KymuxClosedTask {
    fn new(connection: kymux::Connection, msg_proxy: MessageProxy) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        spawn_local(async move {
            tokio::select! {
                _ = stop_rx => {}
                ret = connection.closed() => {
                    let reason = match ret {
                        Ok(()) => DataPlaneCloseReason::Closed,
                        Err(_) => DataPlaneCloseReason::Error,
                    };

                    let ret = msg_proxy.send_async(Message::DataPlaneClosed{ reason }).await;
                    if let Err(err) = ret {
                        warn!("Fail to send DataPlaneClosed: {err:?}");
                    }
                }
            }
        });

        Self { stop_tx }
    }

    async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("ClosedTask already stopped");
        }

        Ok(())
    }
}

async fn connect_webtransport(
    url: String,
    certificate_hash: Option<StartKymuxCertificateHash>,
) -> Result<kymux::Connection> {
    let certificate_hash = certificate_hash.map(|c| kymux::WebTransportCertificateHash {
        hash_algorithm: c.algorithm,
        hash: c.hash,
    });

    Ok(
        kymux::Connection::connect(kymux::ClientConfig::WebTransport {
            url,
            certificate_hash,
        })
        .await?,
    )
}

async fn create_video_decoder_config(config: &VideoSessionConfig) -> Option<VideoDecoderConfig> {
    let supported_video_codecs =
        crate::Client::supported_video_codecs(config.player.software_decode).await;

    let supported_video_codecs: Vec<_> = supported_video_codecs
        .into_iter()
        .filter(|candidate_config| {
            candidate_config.codec == config.codec && candidate_config.yuv444 == config.yuv444
        })
        .collect();

    let video_decoder_config = if supported_video_codecs.len() > 1 {
        supported_video_codecs.into_iter().find(|config| {
            config.hardware_acceleration == web_sys::HardwareAcceleration::PreferHardware
        })?
    } else {
        supported_video_codecs[0].clone()
    };

    Some(video_decoder_config)
}

async fn player_forward_task<Player>(
    type_: &str,
    mut recv: ProtocolRecv<AVPacket>,
    mut player: Player,
    close_requested: CloseRequestState,
) where
    Player: PacketSink,
{
    loop {
        let kypacket = match recv.recv().await {
            Ok(Some(kypacket)) => kypacket,
            Ok(None) => break,
            Err(err) => {
                // Only if the error occurs before the user requested to stop.
                // Otherwise, read errors are expected and should not be
                // reported.
                if !close_requested.is_closed() {
                    warn!("Failed to receive {type_} from Kymux: {err:?}");
                }
                break;
            }
        };

        let packet = kypacket.into();
        let ret = player.send_packet(packet);
        if let Err(err) = ret {
            warn!("Failed to send {type_} frame to player: {err:?}");
        }
    }
}

async fn create_videoplayer(
    kymux: &kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &Option<Vec<StartSessionVideoResponse>>,
    close_requested: &CloseRequestState,
) -> Result<Option<Vec<VideoPlayerHandle>>> {
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
        .unwrap_or(VideoProtocol::Reliable);

    let mut player_list = vec![];

    for config in video_config {
        let endpoint = endpoint_list.get(&config.display_id).ok_or_else(|| {
            warn!("No endpoint found for display {}", config.display_id);
            Error::Rejected
        })?;

        let video_config = create_video_decoder_config(config).await.ok_or_else(|| {
            warn!("No valid decoder config for display {}", config.display_id);
            Error::Rejected
        })?;

        let (video_player, video_player_handle) = VideoPlayer::create(
            backend_config.msg_proxy.clone(),
            &config.player,
            video_config,
            config.player.notify_decoded,
        )?;

        info!("Start video with protocol {video_protocol:?}");

        let endpoint = kymux.connect_video_endpoint(*endpoint, video_protocol)?;
        let recv = endpoint
            .ready()
            .await
            .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

        let cr = close_requested.clone();
        wasm_bindgen_futures::spawn_local(player_forward_task("video", recv, video_player, cr));

        player_list.push(video_player_handle);
    }

    Ok(Some(player_list))
}

async fn create_audioplayer(
    kymux: &kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &Option<StartSessionAudioResponse>,
    close_requested: &CloseRequestState,
) -> Result<Option<AudioPlayerHandle>> {
    let (audio_config, host_config) = match (&backend_config.streaming_config.audio, host_config) {
        (Some(audio_config), Some(host_config)) => (audio_config, host_config),
        (None, None) => return Ok(None),
        _ => {
            error!("Invalid Audio config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    let audio_buffer = audio_config.audio_buffer();
    let audio_player_config = AudioPlayerConfig { audio_buffer };

    let (audio_player, audio_player_handle) =
        match AudioPlayer::create(backend_config.msg_proxy.clone(), &audio_player_config).await {
            Ok((audio_player, audio_player_handle)) => (audio_player, audio_player_handle),
            Err(Error::SharedArrayBufferUndefined) => return Ok(None),
            Err(err) => return Err(err),
        };

    let audio_protocol = backend_config
        .kymux_config
        .audio_protocol
        .unwrap_or(AudioProtocol::Reliable);
    let endpoint = kymux.connect_audio_endpoint(host_config.endpoint, audio_protocol)?;
    let recv = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    let cr = close_requested.clone();
    wasm_bindgen_futures::spawn_local(player_forward_task("audio", recv, audio_player, cr));

    Ok(Some(audio_player_handle))
}

async fn create_player(
    kymux: &mut kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
    close_requested: &CloseRequestState,
) -> Result<Option<Player>> {
    let video_player =
        create_videoplayer(kymux, backend_config, &host_config.video, close_requested).await?;
    let audio_player =
        create_audioplayer(kymux, backend_config, &host_config.audio, close_requested).await?;

    if video_player.is_some() || audio_player.is_some() {
        Ok(Some(Player {
            video: video_player.unwrap_or_default(),
            audio: audio_player,
        }))
    } else {
        Ok(None)
    }
}

async fn input_rx_task(
    mut endpoint_rx: ProtocolRecv<InputPacket>,
    libkynput_tx: BufferProxySender,
) -> Result<()> {
    loop {
        match endpoint_rx.recv().await {
            Ok(Some(pkt)) => {
                use bytes::BufMut;

                let mut packed_pkt = bytes::BytesMut::new();
                packed_pkt.put_u8(pkt.type_);
                packed_pkt.put_slice(&pkt.payload);

                libkynput_tx.send(packed_pkt.to_vec()).await?;
            }
            Ok(None) => {
                info!("Input Rx task: EOS");
                break;
            }
            Err(err) => {
                warn!("Input Rx task error: {err:?}");
                break;
            }
        }
    }

    Ok(())
}

async fn input_tx_task(
    mut endpoint_tx: ProtocolSend<InputPacket>,
    mut libkynput_rx: BufferProxyReceiver,
) -> Result<()> {
    while let Ok(buf) = libkynput_rx.recv().await {
        endpoint_tx
            .send(kymux::InputPacket {
                type_: buf[0],
                payload: bytes::Bytes::copy_from_slice(&buf[1..]),
            })
            .await
            .map_err(|err| Error::KymuxSendFailed { reason: err.0 })?;
    }

    Ok(())
}

async fn start_input(
    kymux: &kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
    msg_proxy: MessageProxy,
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

    // Open endpoint
    let endpoint = kymux.connect_input_endpoint(host_config.endpoint)?;
    let (endpoint_tx, endpoint_rx) = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    // Configure pipeline
    let mut input_proxy = BufferProxy::new();
    let (libkynput_tx, libkynput_rx) = input_proxy.split()?;
    #[allow(clippy::arc_with_non_send_sync)]
    let input_proxy = Arc::new(input_proxy);

    input_proxy.plug_consumer(Arc::downgrade(&input_config.pipeline.get_input_sink()))?;
    input_proxy.run()?;

    let msg_sink = InputMsgSink::new(msg_proxy);
    msg_sink.on_message(InputNetworkStreamMsg::ClientConnected);

    wasm_bindgen_futures::spawn_local(async move {
        tokio::select! {
            ret = input_rx_task(endpoint_rx, libkynput_tx) => {
                info!("End of Input Rx task. Result: {ret:?}");
            },
            ret = input_tx_task(endpoint_tx, libkynput_rx) => {
                info!("End of Input Tx task. Result: {ret:?}");
            },
        }
    });

    Ok(Some(InputService {
        proxy: input_proxy,
        pipeline: input_config.pipeline.clone(),
    }))
}

async fn start_kymux(backend_config: &KymuxBackendConfig<'_>) -> Result<kymux::Connection> {
    // Request remote Kymux start
    let start_response = host_service::start_kymux(
        backend_config.http_client,
        backend_config.kymux_config.backend,
        backend_config.conn_params.port,
    )
    .await?;

    // Start Kymux connection
    let ret = match backend_config.kymux_config.backend {
        KymuxTransportBackend::Webtransport => {
            info!(
                "Connect using Webtransport backend. Port: {port}",
                port = start_response.port
            );

            let url = format!(
                "https://{host}:{port}",
                host = backend_config.conn_params.host,
                port = start_response.port
            );

            connect_webtransport(url, start_response.certificate_hash).await
        }
    };

    ret.map_err(|err| {
        warn!("Failed to connect to DataPlane: {err:?}");
        Error::DataPlaneConnectFailed
    })
}

pub(super) struct KymuxBackend {
    player: Option<Player>,
    input_service: Option<InputService>,
    closed_task: Option<KymuxClosedTask>,
    close_requested: CloseRequestState,
}

impl KymuxBackend {
    pub(super) async fn new(mut backend_config: KymuxBackendConfig<'_>) -> Result<Self> {
        let mut kymux = start_kymux(&backend_config).await?;

        // Tweak configuration in function of local capabilities
        if !AudioPlayer::available() {
            warn!("Disable audio. SharedArrayBuffer not available.");
            backend_config.streaming_config.audio = None;
        }

        // Start stream
        let host_config = host_service::start_stream(&backend_config).await?;

        let close_requested = CloseRequestState::new();

        // Play Video and Audio
        let player =
            create_player(&mut kymux, &backend_config, &host_config, &close_requested).await?;

        // Start input
        let input_service = start_input(
            &kymux,
            &backend_config,
            &host_config,
            backend_config.msg_proxy.clone(),
        )
        .await?;

        // Monitor connection closed
        let closed_task = KymuxClosedTask::new(kymux, backend_config.msg_proxy.clone());

        Ok(Self {
            player,
            input_service,
            closed_task: Some(closed_task),
            close_requested,
        })
    }
}

#[async_trait(?Send)]
impl BackendDriver for KymuxBackend {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let audio_player = self
            .player
            .as_ref()
            .ok_or(Error::NoPlayer)?
            .audio
            .as_ref()
            .ok_or(Error::NoAudioPlayer)?;

        audio_player.set_volume(volume)?;

        Ok(())
    }

    fn audio_set_mute(&self, mute: bool) -> Result<()> {
        let audio_player = self
            .player
            .as_ref()
            .ok_or(Error::NoPlayer)?
            .audio
            .as_ref()
            .ok_or(Error::NoAudioPlayer)?;

        audio_player.set_mute(mute)?;

        Ok(())
    }

    async fn input_service_connected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service
            .pipeline
            .plug_network(input_service.proxy.clone())?;
        input_service.pipeline.start()?;

        Ok(())
    }

    async fn input_service_disconnected(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(closed_task) = self.closed_task.take() {
            let ret = closed_task.stop().await;
            if let Err(err) = ret {
                warn!("Failed to stop KymuxClosedTask: {err:?}");
            }
        }

        if let Some(player) = self.player.take() {
            player.stop().await?;
        }

        if let Some(input_service) = self.input_service.take() {
            info!("Stop input service");
            input_service.pipeline.stop()?;
        }

        self.close_requested.close();

        Ok(())
    }
}
