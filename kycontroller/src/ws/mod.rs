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

mod av;
mod input;
pub(crate) mod requests;

use actix_web::web;
use tokio::sync::{oneshot, Mutex};

use crate::utils::TypeConvert;
use crate::{avservice, inputservice};
use crate::{Controller, DataPlaneBackend};
use crate::{Error, Result};

use kymux::kycom::KyCom;

use av::AVConnection;
use input::InputConnection;

const VIDEO_ENDPOINT_ID: u16 = 0;
const AUDIO_ENDPOINT_ID: u16 = 1;
const INPUT_ENDPOINT_ID: u16 = 2;

/// Lifecycle state of the input WebSocket endpoint.
enum InputState {
    /// Input was not requested for this session.
    NotRequested,
    /// Waiting for a WebSocket client to connect.
    WaitingForConnection(oneshot::Sender<InputConnection>),
    /// Client connected; forwarding task is running.
    Connected(tokio::task::JoinHandle<()>),
}

/// Service handlers started alongside the WebSocket backend.
///
/// Returned from [`WebsocketBackend::start`] so `start_handler` can store
/// them in the client session.
pub(crate) struct StartedServices {
    pub(crate) video: Vec<avservice::Handler>,
    pub(crate) audio: Option<avservice::Handler>,
    pub(crate) input: Option<inputservice::Handler>,
}

pub(crate) struct WebsocketBackend {
    kycom: KyCom,
    /// Oneshot to deliver [`AVConnection`] to the video endpoint when a client connects.
    /// `None` if video was not requested or already connected.
    video: Option<oneshot::Sender<AVConnection>>,
    /// Oneshot to deliver [`AVConnection`] to the audio endpoint when a client connects.
    /// `None` if audio was not requested or already connected.
    audio: Option<oneshot::Sender<AVConnection>>,
    input: InputState,
}

impl WebsocketBackend {
    /// Start kycom, register endpoints, and launch services
    pub(crate) async fn start(
        ctrl: &Controller,
        session_id: uuid::Uuid,
        request: &requests::StartRequest,
    ) -> Result<(Self, StartedServices)> {
        let mut kycom = KyCom::start_on_any_port(9090..9100).await?;

        let (video_sender, video_handler) =
            Self::start_video(&mut kycom, ctrl, session_id, request).await?;
        let (audio_sender, audio_handler) =
            Self::start_audio(&mut kycom, ctrl, session_id, request).await?;
        let (input_sender, input_handler) = Self::start_input(&mut kycom, ctrl, request).await?;

        let backend = Self {
            kycom,
            video: video_sender,
            audio: audio_sender,
            input: match input_sender {
                Some(tx) => InputState::WaitingForConnection(tx),
                None => InputState::NotRequested,
            },
        };

        let services = StartedServices {
            video: video_handler.into_iter().collect(),
            audio: audio_handler,
            input: input_handler,
        };

        Ok((backend, services))
    }

    async fn start_video(
        kycom: &mut KyCom,
        ctrl: &Controller,
        session_id: uuid::Uuid,
        request: &requests::StartRequest,
    ) -> Result<(
        Option<oneshot::Sender<AVConnection>>,
        Option<avservice::Handler>,
    )> {
        let Some(video_request) = request.video.as_ref() else {
            return Ok((None, None));
        };

        let (endpoint, connection_tx) =
            av::AVWebSocketEndpoint::<kymux::types::VideoServerProtocol>::new(VIDEO_ENDPOINT_ID);
        let addr = kycom.register_and_forward(endpoint.into())?;

        let mut avservice_video = avservice::Handler::new(
            avservice::HandlerType::Video,
            Some(session_id),
            ctrl.state_machine_proxy()?,
        );
        avservice_video
            .kymux_start_video(
                &ctrl.process_factory,
                video_request.display_id,
                addr.url(),
                None,
                request.bitrate,
                video_request.codec.clone(),
                video_request.yuv444,
                video_request.intra_refresh,
            )
            .await?;

        Ok((Some(connection_tx), Some(avservice_video)))
    }

    async fn start_audio(
        kycom: &mut KyCom,
        ctrl: &Controller,
        session_id: uuid::Uuid,
        request: &requests::StartRequest,
    ) -> Result<(
        Option<oneshot::Sender<AVConnection>>,
        Option<avservice::Handler>,
    )> {
        if !request.audio {
            return Ok((None, None));
        }

        let (endpoint, connection_tx) =
            av::AVWebSocketEndpoint::<kymux::types::AudioServerProtocol>::new(AUDIO_ENDPOINT_ID);
        let addr = kycom.register_and_forward(endpoint.into())?;

        let mut avservice_audio = avservice::Handler::new(
            avservice::HandlerType::Audio,
            Some(session_id),
            ctrl.state_machine_proxy()?,
        );
        avservice_audio
            .kymux_start_audio(&ctrl.process_factory, addr.url())
            .await?;

        Ok((Some(connection_tx), Some(avservice_audio)))
    }

    async fn start_input(
        kycom: &mut KyCom,
        ctrl: &Controller,
        request: &requests::StartRequest,
    ) -> Result<(
        Option<oneshot::Sender<InputConnection>>,
        Option<inputservice::Handler>,
    )> {
        let Some(inputs_request) = &request.inputs else {
            return Ok((None, None));
        };

        let (endpoint, connection_tx) = input::InputWebSocketEndpoint::new(INPUT_ENDPOINT_ID);
        let addr = kycom.register_and_forward(endpoint.into())?;

        let host_config = kynputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        let service_config = kynputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format.clone(),
            gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
            clipboard: inputs_request.clipboard,
        };

        let mut inputservice = inputservice::Handler::new();
        inputservice
            .start_kymux(
                &ctrl.process_factory,
                host_config,
                service_config,
                addr.url(),
            )
            .await?;

        Ok((Some(connection_tx), Some(inputservice)))
    }

    pub(crate) async fn stop(self) -> Result<()> {
        self.kycom.stop();
        if let InputState::Connected(handle) = self.input {
            if let Err(err) = handle.await {
                log::warn!("Input forward task panicked: {err}");
            }
        }
        Ok(())
    }

    /// Connect service's WebSocket to a kycom endpoint
    pub(crate) async fn connect_service(
        &mut self,
        service: &ServiceType,
        ws_tx: actix_ws::Session,
        ws_rx: actix_ws::MessageStream,
    ) -> Result<()> {
        const COMPONENT_VIDEO: &str = "video";
        const COMPONENT_AUDIO: &str = "audio";
        const COMPONENT_INPUT: &str = "input";

        match service {
            ServiceType::Video => {
                let sender = self
                    .video
                    .take()
                    .ok_or_else(|| Error::WsComponentNotReady {
                        component: COMPONENT_VIDEO.into(),
                    })?;
                av::connect(COMPONENT_VIDEO, sender, ws_tx, ws_rx)
            }
            ServiceType::Audio => {
                let sender = self
                    .audio
                    .take()
                    .ok_or_else(|| Error::WsComponentNotReady {
                        component: COMPONENT_AUDIO.into(),
                    })?;
                av::connect(COMPONENT_AUDIO, sender, ws_tx, ws_rx)
            }
            ServiceType::Input => {
                let InputState::WaitingForConnection(sender) =
                    std::mem::replace(&mut self.input, InputState::NotRequested)
                else {
                    return Err(Error::WsComponentNotReady {
                        component: COMPONENT_INPUT.into(),
                    });
                };
                let handle = input::connect(sender, ws_tx, ws_rx)?;
                self.input = InputState::Connected(handle);
                Ok(())
            }
        }
    }
}

async fn start_handler(
    ctrl: web::Data<Mutex<Controller>>,
    session_id: uuid::Uuid,
    request: &requests::StartRequest,
) -> Result<()> {
    let mut ctrl = ctrl.lock().await;

    // Check if data plane is already opened for this session
    if ctrl.has_data_plane(&session_id)? {
        return Err(Error::DataPlaneAlreadyOpened);
    }

    let (backend, services) = WebsocketBackend::start(&ctrl, session_id, request).await?;

    // Store objects
    let client_session = ctrl.get_session(&session_id)?;
    let Some(streaming_session) = client_session.stream_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    streaming_session.data_plane = Some(DataPlaneBackend::Websocket(backend));
    client_session.services.video = services.video;
    client_session.services.audio = services.audio;
    client_session.services.input = services.input;

    Ok(())
}

#[derive(Debug)]
pub(super) enum ServiceType {
    Video,
    Audio,
    Input,
}

/// Generic WebSocket handler for video, audio, and input services.
///
/// Handles the common WebSocket setup pattern:
/// 1. Validate client/streaming/backend sessions exist
/// 2. Call service-specific routing function
pub(super) async fn start_websocket_handler(
    ctrl: web::Data<Mutex<Controller>>,
    session_id: uuid::Uuid,
    session: actix_ws::Session,
    msg_stream: actix_ws::MessageStream,
    service: ServiceType,
) -> Result<()> {
    let mut ctrl = ctrl.lock().await;

    let client_session = ctrl.get_session(&session_id)?;
    let Some(streaming_session) = client_session.stream_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let Some(DataPlaneBackend::Websocket(backend)) = streaming_session.data_plane.as_mut() else {
        return Err(Error::WsNoRouter);
    };

    backend
        .connect_service(&service, session, msg_stream)
        .await?;

    Ok(())
}
