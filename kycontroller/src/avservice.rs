// Project Kyber: avservice.rs
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

use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};

use kyavservice_types::{Command, Display, Event, VideoCodec};
use libkypc::{
    process::{Process, ProcessFactory},
    CommandSender, Commander, EventReceiver,
};
use tokio::sync::{mpsc, oneshot};

use crate::avservices_manager::StreamId;
use crate::utils;
use crate::{Error, Result, StateMachineEvent, StateMachineProxy};

/// Upper bound on how long the actor waits for an avserver to ACK a
/// fire-and-forget control command (IDR / bitrate). A healthy child acks in
/// milliseconds; [`CommandSender::send`] otherwise blocks until the worker
/// answers, with no built-in timeout. Bounding it here keeps a wedged-but-alive
/// child from blocking the actor forever — which would in turn stall teardown
/// (see [`SharedHandler::stop`]) and let the unbounded command channel grow.
const CMD_ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Overall budget for [`SharedHandler::stop`]. The actor prioritizes `Stop`
/// over any queued control command and its teardown force-kills the child, so
/// this normally completes well within the budget (at most one in-flight
/// command bounded by [`CMD_ACK_TIMEOUT`], plus the child kill). It only
/// elapses if the child is wedged in a way even `child.kill()` can't clear
/// promptly (e.g. an uninterruptible syscall); the actor is then aborted so
/// the AV reaper is never frozen.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

async fn receiver_task(
    type_: HandlerType,
    session_id: Option<uuid::Uuid>,
    // Routing identity of the stream this handler drives. `Some` for the
    // shared/exclusive video & audio streams owned by the AVServicesManager;
    // `None` for the legacy per-session RTP handlers (routed by `session_id`)
    // and the global enumerate/SDP handlers. Forwarded with each event so the
    // StateMachine delivers it to exactly that stream's subscribers.
    stream_id: Option<StreamId>,
    mut receiver: EventReceiver<kyavservice_types::Event>,
    statemachine_proxy: StateMachineProxy,
) {
    while let Ok(event) = receiver.recv().await {
        statemachine_proxy
            .send(StateMachineEvent::AvService {
                session_id,
                stream_id: stream_id.clone(),
                event,
            })
            .await;
    }

    statemachine_proxy
        .send(StateMachineEvent::AvServiceStopped {
            session_id,
            stream_id,
            type_,
        })
        .await;
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum HandlerType {
    EnumerateDisplays,
    VideoSdp,
    Video,
    Audio,
}

pub(crate) struct Handler {
    type_: HandlerType,

    child: Process,
    commander: Commander<Command, Event>,
    sender: CommandSender<Command, Event>,
    receiver_task: Option<utils::Task>,
}

impl Handler {
    async fn spawn_process(
        type_: HandlerType,
        process_factory: &Arc<ProcessFactory>,
    ) -> Result<(Self, EventReceiver<Event>)> {
        debug!("Run AvServer {type_:?}");

        let (child, mut commander): (_, Commander<Command, Event>) = process_factory
            .spawn(utils::get_executable_path("kyavserver"), None)
            .await?;
        let (sender, receiver) = commander.get_sender_receiver()?;

        let this = Self {
            type_,
            child,
            commander,
            sender,
            receiver_task: None,
        };

        Ok((this, receiver))
    }

    fn start_receiver_task(
        &mut self,
        session_id: Option<uuid::Uuid>,
        stream_id: Option<StreamId>,
        statemachine_proxy: StateMachineProxy,
        receiver: EventReceiver<Event>,
    ) {
        self.receiver_task = Some(utils::Task::spawn(receiver_task(
            self.type_,
            session_id,
            stream_id,
            receiver,
            statemachine_proxy,
        )));
    }

    pub(crate) async fn enumerate_displays(
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(Self, Vec<Display>)> {
        let (mut this, receiver) =
            Self::spawn_process(HandlerType::EnumerateDisplays, process_factory).await?;
        let ret = this.sender.send(Command::EnumerateDisplays).await?;

        let Some(Event::DisplayList { displays }) = ret else {
            return Err(Error::DisplayListEmpty);
        };

        this.start_receiver_task(None, None, statemachine_proxy.clone(), receiver);

        Ok((this, displays))
    }

    pub(crate) async fn rtp_generate_video_sdp(
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        yuv444: bool,
        intra_refresh: Option<bool>,
    ) -> Result<String> {
        // Generate SDP
        let (mut this, _) = Self::spawn_process(HandlerType::VideoSdp, process_factory).await?;

        let evt = this
            .sender
            .send(Command::RtpGenerateVideoSdp {
                display_id,
                yuv444,
                intra_refresh,
            })
            .await?;

        // Stop avserver
        this.stop_inner().await?;

        // Unpack results
        let Some(Event::RtpSdpGenerated { sdp }) = evt else {
            return Err(Error::AvServerUnexpectedResponse);
        };

        Ok(sdp)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rtp_start_video(
        session_id: uuid::Uuid,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
        display_id: u32,
        ip: String,
        port: u16,
        bitrate: u32,
        yuv444: bool,
        intra_refresh: Option<bool>,
    ) -> Result<Self> {
        Self::start_inner(
            process_factory,
            statemachine_proxy,
            Some(session_id),
            None,
            HandlerType::Video,
            Command::RtpStartVideo {
                display_id,
                ip,
                port,
                bitrate,
                yuv444,
                intra_refresh,
            },
        )
        .await
    }

    pub(crate) async fn rtp_start_audio(
        session_id: uuid::Uuid,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
        ip: String,
        port: u16,
    ) -> Result<Self> {
        Self::start_inner(
            process_factory,
            statemachine_proxy,
            Some(session_id),
            None,
            HandlerType::Audio,
            Command::RtpStartAudio { ip, port },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn kymux_start_video(
        stream_id: StreamId,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
        display_id: u32,
        uri: String,
        metrics_uri: Option<String>,
        bitrate: u32,
        codec: VideoCodec,
        yuv444: bool,
        intra_refresh: Option<bool>,
    ) -> Result<Self> {
        Self::start_inner(
            process_factory,
            statemachine_proxy,
            None,
            Some(stream_id),
            HandlerType::Video,
            Command::KymuxStartVideo {
                display_id,
                uri,
                metrics_uri,
                bitrate,
                codec,
                yuv444,
                intra_refresh,
            },
        )
        .await
    }

    pub(crate) async fn kymux_start_audio(
        stream_id: StreamId,
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
        uri: String,
    ) -> Result<Self> {
        Self::start_inner(
            process_factory,
            statemachine_proxy,
            None,
            Some(stream_id),
            HandlerType::Audio,
            Command::KymuxStartAudio { uri },
        )
        .await
    }

    async fn start_inner(
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
        session_id: Option<uuid::Uuid>,
        stream_id: Option<StreamId>,
        type_: HandlerType,
        start_cmd: Command,
    ) -> Result<Self> {
        let (mut this, receiver) = Self::spawn_process(type_, process_factory).await?;
        this.sender.send(start_cmd).await?;

        this.start_receiver_task(session_id, stream_id, statemachine_proxy, receiver);

        Ok(this)
    }

    /// Send a stop request to an AvServer
    ///
    /// A basic stop sequence is
    /// 1. Send a stop request
    /// 2. Wait for the process to exit
    ///
    /// However, a stuck AvServer can cause issues at two points
    /// 1. The Stop request can never be answered
    /// 2. The process can never stop correctly
    ///
    /// Both Stop request and Process join have a timeout to make sure
    /// we aren't stuck forever. If one of them timeout, kill the process.
    async fn stop_inner(mut self) -> Result<()> {
        self.receiver_task = None;

        // A command send failure or a process that doesn't stop lead to a process kill
        self.child.kill().await?;

        self.commander.stop().await?;
        debug!("Commander {:?} stopped", self.type_);

        Ok(())
    }

    pub(crate) async fn video_force_idr(&mut self) -> Result<()> {
        debug!("Requesting IDR frame");
        self.sender
            .send_with_timeout(Command::VideoForceIdr, CMD_ACK_TIMEOUT)
            .await?;

        Ok(())
    }

    pub(crate) async fn video_set_bitrate(&mut self, bitrate: i32) -> Result<()> {
        self.sender
            .send(Command::VideoSetBitrate { bitrate })
            .await?;

        Ok(())
    }

    pub(crate) async fn stop(self) -> Result<()> {
        let type_ = self.type_;
        self.stop_inner().await?;
        info!("AvServer {type_:?} exited");

        Ok(())
    }
}

/// Commands accepted by a [`SharedHandler`]'s actor task.
enum SharedCmd {
    VideoForceIdr,
    VideoSetBitrate(i32),
    Stop(oneshot::Sender<Result<()>>),
}

/// Single-owner wrapper around a started [`Handler`] that serializes IPC
/// access through an actor task.
///
/// The shared av_services_manager (Kymux/WS) need concurrent access to a single
/// avservice from the IDR dispatcher, the AV reaper, and HTTP endpoints
/// (set_bitrate). Wrapping the Handler in an actor lets callers post
/// commands via a plain mpsc `Sender::send` (sync, non-awaiting) instead
/// of acquiring an async mutex.
///
/// - `video_force_idr` / `video_set_bitrate`: fire-and-forget; errors are
///   logged inside the actor.
/// - `stop`: async; sends the stop sentinel and awaits the actor's join
///   handle so callers observe the same teardown semantics as
///   `Handler::stop().await`.
pub(crate) struct SharedHandler {
    type_: HandlerType,
    cmd_tx: mpsc::UnboundedSender<SharedCmd>,
    actor_join: Option<utils::Task>,
}

impl SharedHandler {
    /// Consume a started [`Handler`] and spawn its actor task.
    pub(crate) fn spawn(handler: Handler) -> Self {
        let type_ = handler.type_;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let actor_join = utils::Task::spawn(Self::actor_loop(handler, cmd_rx));
        Self {
            type_,
            cmd_tx,
            actor_join: Some(actor_join),
        }
    }

    pub(crate) fn type_(&self) -> HandlerType {
        self.type_
    }

    pub(crate) fn video_force_idr(&self) -> Result<()> {
        self.cmd_tx
            .send(SharedCmd::VideoForceIdr)
            .map_err(|_| Error::StreamerStopped)
    }

    pub(crate) fn video_set_bitrate(&self, bitrate: i32) -> Result<()> {
        self.cmd_tx
            .send(SharedCmd::VideoSetBitrate(bitrate))
            .map_err(|_| Error::StreamerStopped)
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        if self.cmd_tx.send(SharedCmd::Stop(tx)).is_err() {
            // Actor already exited; treat as already-stopped.
            return Ok(());
        }
        let Some(join) = self.actor_join.take() else {
            return Ok(());
        };
        // `Stop` jumps ahead of any queued control command (see `actor_loop`)
        // and the teardown force-kills the child, so the ack normally arrives
        // promptly. The timeout is the last-resort backstop for a child wedged
        // beyond what `child.kill()` can clear in time: abort the actor so the
        // reaper is never frozen. Aborting drops the `Handler` (and thus the
        // `Process` handle) without an explicit kill — `Process` has no
        // `Drop`/`kill_on_drop`, so on that path the orphan is left for the OS
        // to reap once its stuck syscall returns / on host exit. That is the
        // strictly better outcome than freezing every later teardown.
        match tokio::time::timeout(STOP_TIMEOUT, rx).await {
            Ok(Ok(res)) => res,
            Ok(Err(_)) => Err(Error::StreamerStopped),
            Err(_) => {
                warn!(
                    "AvServer {type_:?} stop timed out after {STOP_TIMEOUT:?}; aborting actor",
                    type_ = self.type_(),
                );
                drop(join);
                Err(Error::AvServerStopTimeout)
            }
        }
    }

    async fn actor_loop(mut handler: Handler, mut cmd_rx: mpsc::UnboundedReceiver<SharedCmd>) {
        while let Some(first) = cmd_rx.recv().await {
            // Coalesce the whole backlog in one pass: redundant IDR requests
            // collapse to one (IDR is idempotent), bitrate is last-writer-wins,
            // and a queued Stop takes priority over both. This also keeps the
            // unbounded command channel from growing without bound while the
            // child is wedged — each control command is bounded by
            // `CMD_ACK_TIMEOUT`, so without draining the actor would otherwise
            // fall behind one queued command per timeout under sustained lag.
            let mut want_idr = false;
            let mut bitrate = None;
            let mut stop_ack = None;

            let mut next = Some(first);
            loop {
                let cmd = match next.take() {
                    Some(cmd) => cmd,
                    None => match cmd_rx.try_recv() {
                        Ok(cmd) => cmd,
                        Err(_) => break,
                    },
                };
                match cmd {
                    SharedCmd::VideoForceIdr => want_idr = true,
                    SharedCmd::VideoSetBitrate(b) => bitrate = Some(b),
                    SharedCmd::Stop(ack) => stop_ack = Some(ack),
                }
            }

            // Stop wins: skip the queued control commands and tear down now, so
            // the child is force-killed promptly even behind a large backlog.
            if let Some(ack) = stop_ack {
                let res = handler.stop().await;
                let _ = ack.send(res);
                return;
            }

            if let Some(bitrate) = bitrate {
                if let Err(e) = handler.video_set_bitrate(bitrate).await {
                    warn!("Set bitrate failed: {e:?}");
                }
            }
            if want_idr {
                if let Err(e) = handler.video_force_idr().await {
                    warn!("Force IDR failed: {e:?}");
                }
            }
        }
        // Channel closed without an explicit Stop (every sender dropped).
        if let Err(e) = handler.stop().await {
            warn!("Implicit avservice stop failed: {e:?}");
        }
    }
}
