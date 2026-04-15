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

use std::sync::Arc;

use actix_web::web;
use tokio::sync::{oneshot, Mutex};

use crate::config::SessionMode;
use crate::inputservice;
use crate::kydup;
use crate::utils::TypeConvert;
use crate::{Controller, DataPlaneBackend};
use crate::{Error, Result};

use input::InputConnection;

/// Per-service AV wiring state, picked at start_handler time based on
/// [`SessionMode`].
///
/// `Multi`: a shared avservice feeds kydup; each /websocket/video or
/// /websocket/audio connect adds the WS session as a kydup output. Many
/// clients can subscribe.
///
/// `Mono`: a per-session avservice is plugged directly into kycom, routed to an
/// [`av::AVWebSocketEndpoint`] that stalls until the WS session arrives. The
/// avservice's TCP send buffer absorbs the warm-up; no kydup task is spawned.
enum AvSlot<Dup> {
    Multi(Dup),
    Mono(oneshot::Sender<actix_ws::Session>),
}

pub(crate) struct WebsocketBackend {
    /// Video wiring; `None` if video wasn't requested.
    video: Option<AvSlot<kydup::KydupVideo>>,
    /// Identity of the video stream behind the slot. Used to request a fresh
    /// IDR on that exact stream when the WS video client attaches; for
    /// `Shared` entries this matters because the avservice may be mid-stream;
    /// for `Exclusive` it matters when the warm-up TCP buffer has overflowed
    /// and the initial keyframe has been overwritten. `None` only when video
    /// wasn't requested.
    video_stream_key: Option<crate::avservices_manager::VideoStreamKey>,
    /// Audio wiring; `None` if audio wasn't requested.
    audio: Option<AvSlot<kydup::KydupAudio>>,
    /// Oneshot to deliver [`InputConnection`] to the input endpoint when a client connects.
    /// `None` if input was not requested or already connected.
    input: Option<oneshot::Sender<InputConnection>>,
    /// Join handle for the input WS→channel forwarding task.
    input_forward_handle: Option<tokio::task::JoinHandle<()>>,
}

impl WebsocketBackend {
    pub(crate) async fn stop(self) -> Result<()> {
        // Output handles are cleaned up via StreamingServices.
        if let Some(handle) = self.input_forward_handle {
            if let Err(err) = handle.await {
                log::warn!("Input forward task panicked: {err}");
            }
        }
        Ok(())
    }

    /// Connect a WebSocket to the backend for a given service.
    ///
    /// - `Video`/`Audio` in Multi: wraps the session in [`av::AVWebSocketSendDriver`]
    ///   and adds it as a kydup output. Returns the [`kydup::OutputHandle`].
    /// - `Video`/`Audio` in Mono: hands the session to the
    ///   [`av::AVWebSocketEndpoint`] via its oneshot. kycom's forwarder, which
    ///   was already pumping bytes from the per-session avservice into the
    ///   stalled endpoint, unblocks and the stream flows directly to the WS.
    ///   No kydup task, no [`kydup::OutputHandle`].
    /// - `Input`: delivers the session to the kycom endpoint via oneshot and
    ///   spawns the WS→channel forwarding task; its join handle is kept
    ///   internally for [`Self::stop`] to await.
    ///
    /// `ws_rx` is unused for `Video`/`Audio` (AV is send-only) and dropped.
    ///
    /// Each variant consumes its matching slot (`.take()`); a second call
    /// for the same service returns [`Error::WsComponentNotReady`].
    pub(crate) fn connect_service(
        &mut self,
        service: &ServiceType,
        ws_session: actix_ws::Session,
        ws_rx: actix_ws::MessageStream,
    ) -> Result<Option<kydup::OutputHandle>> {
        // KydupVideo and KydupAudio are distinct types (one per sync mode),
        // so the match arms can't be folded together; each must take its
        // own slot and dispatch.
        match service {
            ServiceType::Video => {
                let slot = self
                    .video
                    .take()
                    .ok_or_else(|| Error::WsComponentNotReady {
                        component: "video".into(),
                    })?;
                drop(ws_rx);
                match slot {
                    AvSlot::Multi(dup) => {
                        let send = kymux::types::ProtocolSend::new(av::AVWebSocketSendDriver::new(
                            ws_session,
                        ));
                        Ok(Some(dup.add_output(send)))
                    }
                    AvSlot::Mono(tx) => {
                        tx.send(ws_session)
                            .map_err(|_| Error::WsComponentNotReady {
                                component: "video".into(),
                            })?;
                        Ok(None)
                    }
                }
            }
            ServiceType::Audio => {
                let slot = self
                    .audio
                    .take()
                    .ok_or_else(|| Error::WsComponentNotReady {
                        component: "audio".into(),
                    })?;
                drop(ws_rx);
                match slot {
                    AvSlot::Multi(dup) => {
                        let send = kymux::types::ProtocolSend::new(av::AVWebSocketSendDriver::new(
                            ws_session,
                        ));
                        Ok(Some(dup.add_output(send)))
                    }
                    AvSlot::Mono(tx) => {
                        tx.send(ws_session)
                            .map_err(|_| Error::WsComponentNotReady {
                                component: "audio".into(),
                            })?;
                        Ok(None)
                    }
                }
            }
            ServiceType::Input => {
                let connection_sender =
                    self.input
                        .take()
                        .ok_or_else(|| Error::WsComponentNotReady {
                            component: "input".into(),
                        })?;
                let handle = input::connect(connection_sender, ws_session, ws_rx)?;
                self.input_forward_handle = Some(handle);
                Ok(None)
            }
        }
    }
}

struct VideoSetup {
    slot: AvSlot<kydup::KydupVideo>,
    /// Identity of the stream behind the slot, used to request a fresh IDR on
    /// exactly that stream when the WS video client attaches. `Shared`
    /// benefits because it joins a mid-flight stream; `Exclusive` benefits
    /// when the encoder's initial keyframe scrolled out of the warm-up TCP
    /// buffer before the WS connected.
    stream_key: crate::avservices_manager::VideoStreamKey,
    subscription: crate::avservices_manager::VideoSubscription,
}

struct AudioSetup {
    slot: AvSlot<kydup::KydupAudio>,
    subscription: crate::avservices_manager::AudioSubscription,
}

/// Build the WS video pipeline for one session. Mode-agnostic at the call
/// site: the only branch is on the `VideoTopology` we hand to `av_services_manager`,
/// and on whether the slot is `AvSlot::Multi(dup)` (caller attaches outputs
/// to kydup at `/websocket/video` time) or `AvSlot::Mono(connection_tx)`
/// (avservice already plumbed straight into an `AVWebSocketEndpoint` that
/// stalls until `/websocket/video` arrives).
async fn setup_video(
    ctrl: &mut Controller,
    session_id: uuid::Uuid,
    request: &requests::StartRequest,
    video_request: &requests::StartRequestVideo,
    process_factory: &Arc<libkypc::process::ProcessFactory>,
    statemachine_proxy: crate::StateMachineProxy,
    mode: SessionMode,
) -> Result<VideoSetup> {
    let config = crate::avservices_manager::VideoStreamConfig {
        bitrate: request.bitrate,
        codec: video_request.codec.clone(),
        yuv444: video_request.yuv444,
        intra_refresh: video_request.intra_refresh,
        video_stream_count: 1,
    };
    let display_id = video_request.display_id;

    match mode {
        SessionMode::Multi => {
            let kycom = ctrl.kycom.as_mut().expect("ensure_kycom guarantees Some");
            let topology = crate::avservices_manager::VideoTopology::Shared { display_id };
            let acq = ctrl
                .av_services_manager
                .acquire_video(
                    kycom,
                    topology,
                    config,
                    false,
                    process_factory,
                    statemachine_proxy,
                )
                .await?;
            let dup = acq.dup.expect("Shared topology guarantees Some(dup)");
            Ok(VideoSetup {
                slot: AvSlot::Multi(dup),
                stream_key: acq.subscription.key().clone(),
                subscription: acq.subscription,
            })
        }
        SessionMode::Mono => {
            let video_eid = ctrl.alloc_ws_endpoint_id();
            let kycom = ctrl.kycom.as_mut().expect("ensure_kycom guarantees Some");
            let (endpoint, connection_tx) =
                av::AVWebSocketEndpoint::<kymux::types::VideoServerProtocol>::new(video_eid);
            let kycom_uri = kycom
                .register_and_forward(endpoint.into())
                .map_err(|e| Error::IO { source: e })?
                .url();
            let topology = crate::avservices_manager::VideoTopology::Exclusive {
                session_id,
                display_id,
                kycom_uri,
            };
            let acq = ctrl
                .av_services_manager
                .acquire_video(
                    kycom,
                    topology,
                    config,
                    false,
                    process_factory,
                    statemachine_proxy,
                )
                .await?;
            debug_assert!(acq.dup.is_none());
            Ok(VideoSetup {
                slot: AvSlot::Mono(connection_tx),
                stream_key: acq.subscription.key().clone(),
                subscription: acq.subscription,
            })
        }
    }
}

/// Audio counterpart of [`setup_video`].
async fn setup_audio(
    ctrl: &mut Controller,
    session_id: uuid::Uuid,
    process_factory: &Arc<libkypc::process::ProcessFactory>,
    statemachine_proxy: crate::StateMachineProxy,
    mode: SessionMode,
) -> Result<AudioSetup> {
    match mode {
        SessionMode::Multi => {
            let kycom = ctrl.kycom.as_mut().expect("ensure_kycom guarantees Some");
            let topology = crate::avservices_manager::AudioTopology::Shared;
            let acq = ctrl
                .av_services_manager
                .acquire_audio(kycom, topology, process_factory, statemachine_proxy)
                .await?;
            let dup = acq.dup.expect("Shared topology guarantees Some(dup)");
            Ok(AudioSetup {
                slot: AvSlot::Multi(dup),
                subscription: acq.subscription,
            })
        }
        SessionMode::Mono => {
            let audio_eid = ctrl.alloc_ws_endpoint_id();
            let kycom = ctrl.kycom.as_mut().expect("ensure_kycom guarantees Some");
            let (endpoint, connection_tx) =
                av::AVWebSocketEndpoint::<kymux::types::AudioServerProtocol>::new(audio_eid);
            let kycom_uri = kycom
                .register_and_forward(endpoint.into())
                .map_err(|e| Error::IO { source: e })?
                .url();
            let topology = crate::avservices_manager::AudioTopology::Exclusive {
                session_id,
                kycom_uri,
            };
            let acq = ctrl
                .av_services_manager
                .acquire_audio(kycom, topology, process_factory, statemachine_proxy)
                .await?;
            debug_assert!(acq.dup.is_none());
            Ok(AudioSetup {
                slot: AvSlot::Mono(connection_tx),
                subscription: acq.subscription,
            })
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

    // Ensure shared kycom is running
    ctrl.ensure_kycom().await?;

    // Extract values before borrows
    let process_factory = ctrl.process_factory.clone();
    let statemachine_proxy = ctrl.state_machine_proxy()?;
    let mode = ctrl.config.session_mode();

    let video_setup = if let Some(video_request) = request.video.as_ref() {
        Some(
            setup_video(
                &mut ctrl,
                session_id,
                request,
                video_request,
                &process_factory,
                statemachine_proxy.clone(),
                mode,
            )
            .await?,
        )
    } else {
        None
    };

    let audio_setup = if request.audio {
        Some(
            setup_audio(
                &mut ctrl,
                session_id,
                &process_factory,
                statemachine_proxy.clone(),
                mode,
            )
            .await?,
        )
    } else {
        None
    };

    // Input: per-session via kycom (not shared via kydup)
    let (input_sender, inputservice) = if let Some(inputs_request) = &request.inputs {
        let input_eid = ctrl.alloc_ws_endpoint_id();

        let host_config = kynputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };
        let service_config = kynputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format.clone(),
            gamepad_force_xbox360: inputs_request.gamepad_force_xbox360,
            clipboard: inputs_request.clipboard,
        };

        let kycom = ctrl.kycom.as_mut().expect("ensure_kycom guarantees Some");
        let (endpoint, connection_tx) = input::InputWebSocketEndpoint::new(input_eid);
        let addr = kycom
            .register_and_forward(endpoint.into())
            .map_err(|e| Error::IO { source: e })?;

        let (inputservice, _) = inputservice::Handler::start_kymux(
            &process_factory,
            host_config,
            service_config,
            addr.url(),
        )
        .await?;

        (Some(connection_tx), Some(inputservice))
    } else {
        (None, None)
    };

    // Split slot/subscription and store both. Slots go into the backend so the
    // /websocket/video|audio handler can dispatch on connect; subscriptions go into
    // the session's services so they live as long as the session, and so
    // their Drop signals the av_services_manager reaper when the session ends.
    let (video_slot, video_stream_key, video_subscription) = match video_setup {
        Some(s) => (Some(s.slot), Some(s.stream_key), Some(s.subscription)),
        None => (None, None, None),
    };
    let (audio_slot, audio_subscription) = match audio_setup {
        Some(s) => (Some(s.slot), Some(s.subscription)),
        None => (None, None),
    };

    let client_session = ctrl.get_session(&session_id)?;
    let Some(streaming_session) = client_session.stream_session.as_mut() else {
        return Err(Error::ControlPlaneNotOpened);
    };

    let backend = WebsocketBackend {
        video: video_slot,
        video_stream_key,
        audio: audio_slot,
        input: input_sender,
        input_forward_handle: None,
    };
    streaming_session.data_plane = Some(DataPlaneBackend::Websocket(backend));

    client_session.services.input = inputservice;
    if let Some(subscription) = video_subscription {
        client_session
            .services
            .video_subscriptions
            .push(subscription);
    }
    if let Some(subscription) = audio_subscription {
        client_session.services.audio_subscription = Some(subscription);
    }

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

    // Phase 1: under the per-session mutable borrow, run connect_service
    // and capture (if Video) the stream key so we can request a fresh IDR
    // afterwards on `av_services_manager`.
    let idr_key = {
        let client_session = ctrl.get_session(&session_id)?;
        let (handle_opt, idr_key) = {
            let streaming_session = client_session
                .stream_session
                .as_mut()
                .ok_or(Error::ControlPlaneNotOpened)?;
            let backend = match &mut streaming_session.data_plane {
                Some(DataPlaneBackend::Websocket(b)) => b,
                _ => return Err(Error::WsNoRouter),
            };
            let handle = backend.connect_service(&service, session, msg_stream)?;
            let idr_key = if matches!(service, ServiceType::Video) {
                backend.video_stream_key.clone()
            } else {
                None
            };
            (handle, idr_key)
        };
        if let Some(handle) = handle_opt {
            client_session.services.output_handles.push(handle);
        }
        idr_key
    };

    // Phase 2: client_session borrow released, free to touch av_services_manager.
    // Force an IDR to give the joining client a sync point: under intra-refresh
    // (the default) the encoder emits no periodic keyframes, so this is the only
    // frame a client in keyframe-sync mode can sync on. The WS output subscribes
    // eagerly (add_output), so forcing the IDR here, after the subscribe, is
    // correct. (The kymux backend instead forces its IDR from the kydup
    // post-subscribe hook, since it subscribes lazily; see
    // `kymux_controller::start_video`.)
    if let Some(key) = idr_key {
        ctrl.av_services_manager.request_idr(key);
    }

    Ok(())
}
