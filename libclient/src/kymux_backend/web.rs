use std::rc::Rc;
use std::sync::Arc;

use log::{error, info, warn};

use kymux::{AVPacket, InputPacket, ProtocolEndpoint, ProtocolRecv, ProtocolSend, VideoProtocol};
use libkynput::bufferproxy::{BufferProxy, BufferProxyReceiver, BufferProxySender};
use libkynput::{InputNetworkStream, InputNetworkStreamMsg, InputNetworkStreamObserver};

use super::KymuxConfig;
use crate::kymux_backend::host_service::StartKymuxCertificateHash;
use crate::platform::player::{AudioPlayer, FrameSink, VideoDecoderConfig, VideoPlayer};
use crate::platform::InputMsgSink;
use crate::{Error, Result};
use crate::{
    InputSessionConfig, MessageProxy, PlayerConfig, StreamingSessionConfig, VideoSessionConfig,
};

pub(super) struct InputService {
    pub(super) proxy: Arc<BufferProxy>,
    pub(super) pipeline: Arc<libkynput::InputPipeline>,
}

pub(super) struct Player {
    pub(super) video: Option<Rc<VideoPlayer>>,
    pub(super) audio: Option<Rc<AudioPlayer>>,
}

impl Player {
    pub(super) async fn stop(self) -> Result<()> {
        if let Some(video_player) = self.video {
            info!("Stop video player");
            let ret = video_player.stop().await;
            if let Err(err) = ret {
                warn!("Video player stop failed: {err:?}");
            }
        }

        if let Some(_audio_player) = self.audio {
            info!("Stop audio player");
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

async fn create_video_decoder_config(
    player_config: &PlayerConfig,
    config: &Option<VideoSessionConfig>,
) -> Option<VideoDecoderConfig> {
    let Some(config) = config.as_ref() else {
        return None;
    };

    let supported_video_codecs =
        crate::Client::supported_video_codecs(player_config.software_decode).await;

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
    player: Rc<Player>,
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
    video_endpoint: u16,
    video_protocol: VideoProtocol,
    player_config: &PlayerConfig,
    video_config: VideoDecoderConfig,
    msg_proxy: MessageProxy,
) -> Result<Rc<VideoPlayer>> {
    let video_player = Rc::new(VideoPlayer::new(
        &player_config.canvas,
        msg_proxy.clone(),
        video_config,
    )?);

    info!("Start video with protocol {video_protocol:?}");

    let endpoint = kymux.connect_video_endpoint(video_endpoint, video_protocol)?;
    let recv = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    wasm_bindgen_futures::spawn_local(player_forward_task("video", recv, video_player.clone()));

    Ok(video_player)
}

async fn create_audioplayer(
    kymux: &kymux::Connection,
    _kymux_config: &KymuxConfig,
    audio_endpoint: u16,
    _player_config: &PlayerConfig,
    msg_proxy: MessageProxy,
) -> Result<Rc<AudioPlayer>> {
    let audio_player = Rc::new(AudioPlayer::new(msg_proxy).await?);

    let endpoint = kymux.connect_audio_endpoint(audio_endpoint)?;
    let recv = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    wasm_bindgen_futures::spawn_local(player_forward_task("audio", recv, audio_player.clone()));

    Ok(audio_player)
}

pub(super) async fn create_player(
    kymux: &mut kymux::Connection,
    kymux_config: &KymuxConfig,
    player_config: &PlayerConfig,
    streaming_config: &StreamingSessionConfig,
    video_endpoint: &Option<u16>,
    audio_endpoint: &Option<u16>,
    msg_proxy: MessageProxy,
) -> Result<Option<Player>> {
    // Create Video player
    let video_config = create_video_decoder_config(player_config, &streaming_config.video).await;

    let video_player = match (video_config, video_endpoint, kymux_config.video_protocol) {
        (Some(video_config), Some(video_endpoint), Some(video_protocol)) => {
            let video_player = create_videoplayer(
                kymux,
                *video_endpoint,
                video_protocol,
                player_config,
                video_config,
                msg_proxy.clone(),
            )
            .await?;

            Some(video_player)
        }
        (None, None, None) => None,
        _ => {
            error!("Invalid Video config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    // Create Audio player
    let audio_player = match (streaming_config.audio, audio_endpoint) {
        (true, Some(audio_endpoint)) => {
            let audio_player = create_audioplayer(
                kymux,
                kymux_config,
                *audio_endpoint,
                player_config,
                msg_proxy.clone(),
            )
            .await?;

            Some(audio_player)
        }
        (false, None) => None,
        _ => {
            error!("Invalid Audio config. Should either get a config and an endpoint, or none of them.");
            return Err(Error::InvalidState);
        }
    };

    if video_player.is_some() || audio_player.is_some() {
        Ok(Some(Player {
            video: video_player,
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
