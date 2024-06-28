use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use log::{error, info, warn};
use tokio::sync::oneshot;
use wasm_bindgen_futures::spawn_local;

use kymux::{AVPacket, InputPacket, ProtocolEndpoint, ProtocolRecv, ProtocolSend, VideoProtocol};
use libkynput::bufferproxy::{BufferProxy, BufferProxyReceiver, BufferProxySender};
use libkynput::{InputNetworkStream, InputNetworkStreamMsg, InputNetworkStreamObserver};

use super::KymuxBackendConfig;
use crate::kymux_backend::host_service::{
    StartKymuxCertificateHash, StartSessionAudioResponse, StartSessionResponse,
    StartSessionVideoResponse,
};
use crate::platform::player::{
    AudioPlayer, AudioPlayerHandle, FrameSink, VideoDecoderConfig, VideoPlayer, VideoPlayerHandle,
};
use crate::platform::InputMsgSink;
use crate::{Error, Result};
use crate::{InputSessionConfig, MessageProxy, VideoSessionConfig};

pub(super) struct InputService {
    pub(super) proxy: Arc<BufferProxy>,
    pub(super) pipeline: Arc<libkynput::InputPipeline>,
}

pub(super) struct Player {
    pub(super) video: Vec<VideoPlayerHandle>,
    pub(super) audio: Option<AudioPlayerHandle>,
}

impl Player {
    pub(super) async fn stop(self) -> Result<()> {
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

pub(super) struct KymuxClosedTask {
    stop_tx: oneshot::Sender<()>,
}

impl KymuxClosedTask {
    pub(super) fn new<F>(task_entry: F) -> Self
    where
        F: Future<Output = Result<()>> + 'static,
    {
        let (stop_tx, stop_rx) = oneshot::channel();

        spawn_local(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                ret = task_entry => {
                    if let Err(err) = ret {
                        warn!("ClosedTask task ended with error {err:?}");
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

pub(super) async fn connect_webtransport(
    url: String,
    certificate_hash: StartKymuxCertificateHash,
) -> Result<kymux::Connection> {
    Ok(
        kymux::Connection::connect(kymux::ClientConfig::WebTransport {
            url,
            certificate_hash_algorithm: certificate_hash.algorithm,
            certificate_hash: certificate_hash.hash,
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
) where
    Player: FrameSink,
{
    loop {
        let packet = match recv.recv().await {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(err) => {
                warn!("Failed to receive {type_} from Kymux: {err:?}");
                break;
            }
        };

        let frame = match packet {
            AVPacket::Codec(packet) => {
                let header = packet.header.serialize();

                let buffer = js_sys::Uint8Array::new_with_length(header.len() as _);
                buffer.copy_from(&header);
                buffer
            }
            AVPacket::Media(packet) => {
                let header = packet.header.serialize();
                let payload: Vec<u8> = packet.payload.to_vec();

                let buffer =
                    js_sys::Uint8Array::new_with_length((header.len() + payload.len()) as _);

                let header_view = buffer.subarray(0, header.len() as _);
                header_view.copy_from(&header);

                let payload_view = buffer.subarray(header.len() as _, buffer.length());
                payload_view.copy_from(&payload);

                buffer
            }
        };

        let ret = player.send_frame(frame.buffer());
        if let Err(err) = ret {
            warn!("Failed to send {type_} frame to player: {err:?}");
        }
    }
}

async fn create_videoplayer(
    kymux: &kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &Option<Vec<StartSessionVideoResponse>>,
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
            &config.player.canvas,
            backend_config.msg_proxy.clone(),
            video_config,
        )?;

        info!("Start video with protocol {video_protocol:?}");

        let endpoint = kymux.connect_video_endpoint(*endpoint, video_protocol)?;
        let recv = endpoint
            .ready()
            .await
            .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

        wasm_bindgen_futures::spawn_local(player_forward_task("video", recv, video_player));

        player_list.push(video_player_handle);
    }

    Ok(Some(player_list))
}

async fn create_audioplayer(
    kymux: &kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &Option<StartSessionAudioResponse>,
) -> Result<Option<AudioPlayerHandle>> {
    let host_config = match (backend_config.streaming_config.audio, host_config) {
        (true, Some(host_config)) => host_config,
        (false, None) => return Ok(None),
        _ => {
            error!("Invalid Audio config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    let (audio_player, audio_player_handle) =
        AudioPlayer::create(backend_config.msg_proxy.clone()).await?;

    let endpoint = kymux.connect_audio_endpoint(host_config.endpoint)?;
    let recv = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    wasm_bindgen_futures::spawn_local(player_forward_task("audio", recv, audio_player));

    Ok(Some(audio_player_handle))
}

pub(super) async fn create_player(
    kymux: &mut kymux::Connection,
    backend_config: &KymuxBackendConfig<'_>,
    host_config: &StartSessionResponse,
) -> Result<Option<Player>> {
    let video_player = create_videoplayer(kymux, backend_config, &host_config.video).await?;
    let audio_player = create_audioplayer(kymux, backend_config, &host_config.audio).await?;

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

pub(super) async fn start_input(
    kymux: &kymux::Connection,
    config: &InputSessionConfig,
    endpoint: u16,
    msg_proxy: MessageProxy,
) -> Result<Option<InputService>> {
    // Open endpoint
    let endpoint = kymux.connect_input_endpoint(endpoint)?;
    let (endpoint_tx, endpoint_rx) = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    // Configure pipeline
    let mut input_proxy = BufferProxy::new();
    let (libkynput_tx, libkynput_rx) = input_proxy.split()?;
    let input_proxy = Arc::new(input_proxy);

    input_proxy.plug_consumer(Arc::downgrade(&config.pipeline.get_input_sink()))?;
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
        pipeline: config.pipeline.clone(),
    }))
}

pub(super) struct Inner {
    pub(super) player: Option<Player>,
    pub(super) input_service: Option<InputService>,
    pub(super) closed_task: Option<KymuxClosedTask>,
}

impl Inner {
    pub(super) async fn input_service_connected(&mut self) -> Result<()> {
        let input_service = self.input_service.as_mut().ok_or(Error::NoInputService)?;

        input_service
            .pipeline
            .plug_network(input_service.proxy.clone())?;
        input_service.pipeline.start()?;

        Ok(())
    }

    pub(super) async fn input_service_disconnected(&mut self) -> Result<()> {
        Ok(())
    }

    pub(super) async fn stop(&mut self) -> Result<()> {
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

        Ok(())
    }
}
