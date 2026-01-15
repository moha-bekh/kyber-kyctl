// Project Kyber: web.rs
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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};

use kymux::types;
use kynput::bufferproxy::{BufferProxy, BufferProxyReceiver, BufferProxySender};
use kynput::{InputNetworkStream, InputNetworkStreamMsg, InputNetworkStreamObserver};

use super::{KymuxBackendConfig, PlatformEndpoints};
use crate::platform::message::{AudioMessageForwarder, VideoMessageForwarder};
use crate::platform::player::{
    self, AudioPlayer, AudioPlayerCreationError, AudioPlayerHandle, PacketSink, VideoDecoderConfig,
    VideoPlayer, VideoPlayerHandle,
};
use crate::platform::InputMsgSink;
use crate::{BackendDriver, MessageSender, VideoSessionConfig};
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
    pipeline: Arc<kynput::InputPipeline>,
}

struct Player {
    video: Vec<VideoPlayerHandle>,
    audio: Option<AudioPlayerHandle>,
}

impl Player {
    async fn stop(self) {
        for mut video_player in self.video {
            info!("Stop video player");
            video_player.stop().await;
        }

        if let Some(mut audio_player) = self.audio {
            info!("Stop audio player");
            audio_player.stop().await;
        }
    }
}

async fn create_video_decoder_config(config: &VideoSessionConfig) -> Option<VideoDecoderConfig> {
    let supported_video_codecs =
        player::supported_video_codecs(config.player.software_decode).await;

    let codec = config.codec.into();
    let supported_video_codecs: Vec<_> = supported_video_codecs
        .into_iter()
        .filter(|candidate_config| {
            candidate_config.codec == codec && candidate_config.yuv444 == config.yuv444
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
    mut recv: types::ProtocolRecv<types::AVPacket>,
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
    backend_config: &KymuxBackendConfig<'_>,
    close_requested: &CloseRequestState,
    metrics: bool,
    endpoints: Vec<types::VideoClientEndpoint>,
) -> Result<Option<Vec<VideoPlayerHandle>>> {
    if endpoints.is_empty() {
        return Ok(None);
    };

    let video_config = &backend_config.streaming_config.video;

    assert!(!video_config.is_empty(), "called without video config");

    // By construction, one endpoint is created for each video_config.
    // If this is not the case, this is a programming error.
    assert_eq!(
        endpoints.len(),
        video_config.len(),
        "Mismatch between video_config and endpoints"
    );

    let mut player_list = vec![];

    for (config, endpoint) in video_config.iter().zip(endpoints.into_iter()) {
        let video_config = create_video_decoder_config(config).await.ok_or_else(|| {
            warn!("No valid decoder config for display {}", config.display_id);
            Error::Rejected
        })?;

        let listener = Box::new(VideoMessageForwarder {
            message_sender: backend_config.msg_sender.clone(),
            source_id: config.display_id,
        });
        let (video_player, video_player_handle) = VideoPlayer::create(
            listener,
            &config.player.clone().into(),
            video_config,
            metrics,
        )?;

        info!("Start video");

        let protocol = endpoint
            .ready()
            .await
            .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

        let cr = close_requested.clone();
        wasm_bindgen_futures::spawn_local(player_forward_task(
            "video",
            protocol.recv,
            video_player,
            cr,
        ));

        player_list.push(video_player_handle);
    }

    Ok(Some(player_list))
}

async fn create_audioplayer(
    backend_config: &KymuxBackendConfig<'_>,
    close_requested: &CloseRequestState,
    endpoint: Option<types::AudioClientEndpoint>,
) -> Result<Option<AudioPlayerHandle>> {
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };

    let audio_config = backend_config
        .streaming_config
        .audio
        .as_ref()
        .expect("called without audio config");

    let audio_buffer = audio_config.audio_buffer();
    let audio_player_config = player::AudioPlayerConfig { audio_buffer };

    let listener = Box::new(AudioMessageForwarder {
        message_sender: backend_config.msg_sender.clone(),
    });
    let (audio_player, audio_player_handle) =
        match AudioPlayer::create(listener, &audio_player_config).await {
            Ok((audio_player, audio_player_handle)) => (audio_player, audio_player_handle),
            Err(AudioPlayerCreationError::SharedArrayBufferNotAvailable) => return Ok(None),
            Err(AudioPlayerCreationError::Other(err)) => return Err(err.into()),
        };

    let protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;

    let cr = close_requested.clone();
    wasm_bindgen_futures::spawn_local(player_forward_task(
        "audio",
        protocol.recv,
        audio_player,
        cr,
    ));

    Ok(Some(audio_player_handle))
}

async fn create_player(
    backend_config: &KymuxBackendConfig<'_>,
    close_requested: &CloseRequestState,
    metrics: bool,
    video_endpoints: Vec<types::VideoClientEndpoint>,
    audio_endpoint: Option<types::AudioClientEndpoint>,
) -> Result<Option<Player>> {
    let video_player =
        create_videoplayer(backend_config, close_requested, metrics, video_endpoints).await?;
    let audio_player = create_audioplayer(backend_config, close_requested, audio_endpoint).await?;

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
    mut endpoint_rx: types::ProtocolRecv<types::InputPacket>,
    kynput_tx: BufferProxySender,
) -> Result<()> {
    loop {
        match endpoint_rx.recv().await {
            Ok(Some(pkt)) => {
                use bytes::BufMut;

                let mut packed_pkt = bytes::BytesMut::new();
                packed_pkt.put_u8(pkt.type_);
                packed_pkt.put_slice(&pkt.payload);

                kynput_tx.send(packed_pkt.to_vec()).await?;
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
    mut endpoint_tx: types::ProtocolSend<types::InputPacket>,
    mut kynput_rx: BufferProxyReceiver,
) -> Result<()> {
    while let Ok(buf) = kynput_rx.recv().await {
        endpoint_tx
            .send(types::InputPacket {
                type_: buf[0],
                payload: bytes::Bytes::copy_from_slice(&buf[1..]),
            })
            .await
            .map_err(|err| Error::KymuxSendFailed { reason: err.0 })?;
    }

    Ok(())
}

async fn start_input(
    backend_config: &KymuxBackendConfig<'_>,
    msg_sender: MessageSender,
    endpoint: Option<types::InputEndpoint>,
) -> Result<Option<InputService>> {
    let Some(endpoint) = endpoint else {
        return Ok(None);
    };

    let input_config = backend_config
        .streaming_config
        .input
        .as_ref()
        .expect("called without input config");

    // Open endpoint
    let protocol = endpoint
        .ready()
        .await
        .map_err(|err| Error::KymuxReadyFailed { reason: err.0 })?;
    let (endpoint_tx, endpoint_rx) = (protocol.send, protocol.recv);

    // Configure pipeline
    let mut input_proxy = BufferProxy::new();
    let (kynput_tx, kynput_rx) = input_proxy.split()?;
    #[allow(clippy::arc_with_non_send_sync)]
    let input_proxy = Arc::new(input_proxy);

    input_proxy.plug_consumer(Arc::downgrade(&input_config.pipeline.get_input_sink()))?;
    input_proxy.run()?;

    let msg_sink = InputMsgSink::new(msg_sender);
    msg_sink.on_message(InputNetworkStreamMsg::ClientConnected);

    wasm_bindgen_futures::spawn_local(async move {
        tokio::select! {
            ret = input_rx_task(endpoint_rx, kynput_tx) => {
                info!("End of Input Rx task. Result: {ret:?}");
            },
            ret = input_tx_task(endpoint_tx, kynput_rx) => {
                info!("End of Input Tx task. Result: {ret:?}");
            },
        }
    });

    Ok(Some(InputService {
        proxy: input_proxy,
        pipeline: input_config.pipeline.clone(),
    }))
}

pub(super) struct KymuxWeb {
    player: Option<Player>,
    input_service: Option<InputService>,
    close_requested: CloseRequestState,
}

impl KymuxWeb {
    pub(super) async fn new(
        backend_config: &KymuxBackendConfig<'_>,
        mut endpoints: PlatformEndpoints,
        has_metrics: bool,
    ) -> Result<Self> {
        // Tweak configuration in function of local capabilities
        if !AudioPlayer::available() {
            warn!("Disable audio. SharedArrayBuffer not available.");
            endpoints.audio = None;
        }

        let close_requested = CloseRequestState::new();

        // Play Video and Audio
        let player = create_player(
            backend_config,
            &close_requested,
            has_metrics,
            endpoints.video,
            endpoints.audio,
        )
        .await?;

        // Start input
        let input_service = start_input(
            backend_config,
            backend_config.msg_sender.clone(),
            endpoints.input,
        )
        .await?;

        Ok(Self {
            player,
            input_service,
            close_requested,
        })
    }
}

#[async_trait(?Send)]
impl BackendDriver for KymuxWeb {
    fn audio_set_volume(&self, volume: f32) -> Result<()> {
        let audio_player = self
            .player
            .as_ref()
            .ok_or(Error::NoPlayer)?
            .audio
            .as_ref()
            .ok_or(Error::NoAudioPlayer)?;

        audio_player.set_volume(volume);

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

        audio_player.set_mute(mute);

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
        if let Some(player) = self.player.take() {
            player.stop().await;
        }

        if let Some(input_service) = self.input_service.take() {
            info!("Stop input service");
            input_service.pipeline.stop()?;
        }

        self.close_requested.close();

        Ok(())
    }
}
