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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use tokio::{
    sync::{mpsc, oneshot},
    task, time,
};

use kyavservice_types::{Command, Display, Event, VideoCodec};
use libkypc::{
    process::{Process, ProcessFactory},
    CommandSender, Commander, EventReceiver,
};

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

struct ReceiverTask {
    type_: HandlerType,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl ReceiverTask {
    fn new(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        stream_id: Option<StreamId>,
        receiver: EventReceiver<Event>,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(
            type_,
            session_id,
            stream_id,
            receiver,
            statemachine_proxy,
            stop_rx,
        ));

        Self {
            type_,
            handle,
            stop_tx,
        }
    }

    async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("{:?} receiver task stop send request failed", self.type_);
        }

        self.handle.await.map_err(|err| {
            warn!("{:?} receiver task panicked: {err:?}", self.type_);
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    async fn task_entry(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        stream_id: Option<StreamId>,
        receiver: EventReceiver<kyavservice_types::Event>,
        statemachine_proxy: StateMachineProxy,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_receiver(type_, session_id, stream_id, receiver, statemachine_proxy) => {}
        }
    }

    async fn process_receiver(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
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
}

struct Inner {
    child: Process,
    commander: Commander<Command, Event>,
    sender: CommandSender<Command, Event>,
    receiver: Option<EventReceiver<Event>>,
    receiver_task: Option<ReceiverTask>,
}

impl Inner {
    fn start_receiver_task(
        &mut self,
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        stream_id: Option<StreamId>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<()> {
        let receiver = self.receiver.take().ok_or(Error::AvServerNoReceiver)?;
        self.receiver_task = Some(ReceiverTask::new(
            type_,
            session_id,
            stream_id,
            receiver,
            statemachine_proxy,
        ));

        Ok(())
    }
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
    session_id: Option<uuid::Uuid>,
    /// Routing identity of the stream this handler drives. `Some` for the
    /// shared/exclusive video & audio streams owned by the AVServicesManager;
    /// `None` for the legacy per-session RTP handlers (routed by `session_id`)
    /// and the global enumerate/SDP handlers. Forwarded with each event so the
    /// StateMachine delivers it to exactly that stream's subscribers.
    stream_id: Option<StreamId>,
    avserver_path: PathBuf,
    statemachine_proxy: StateMachineProxy,
    inner: Option<Inner>,
}

impl Handler {
    pub(crate) fn new(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        stream_id: Option<StreamId>,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        Self {
            type_,
            session_id,
            stream_id,
            avserver_path: utils::get_executable_path("kyavserver"),
            inner: None,
            statemachine_proxy,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    pub(crate) fn type_(&self) -> HandlerType {
        self.type_
    }

    async fn spawn_process(&self, process_factory: &Arc<ProcessFactory>) -> Result<Inner> {
        debug!("Run AvServer {type_:?}", type_ = self.type_);

        let (child, mut commander): (_, Commander<Command, Event>) =
            process_factory.spawn(&self.avserver_path, None).await?;
        let (sender, receiver) = commander.get_sender_receiver()?;

        Ok(Inner {
            child,
            commander,
            sender,
            receiver: Some(receiver),
            receiver_task: None,
        })
    }

    pub(crate) async fn enumerate_displays(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
    ) -> Result<Vec<Display>> {
        let mut inner = self.spawn_process(process_factory).await?;
        let ret = inner.sender.send(Command::EnumerateDisplays).await?;

        let Some(Event::DisplayList { displays }) = ret else {
            return Err(Error::DisplayListEmpty);
        };

        inner.start_receiver_task(
            self.type_,
            self.session_id,
            self.stream_id.clone(),
            self.statemachine_proxy.clone(),
        )?;
        self.inner = Some(inner);

        Ok(displays)
    }

    pub(crate) async fn rtp_generate_video_sdp(
        &self,
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        yuv444: bool,
        intra_refresh: Option<bool>,
    ) -> Result<String> {
        // Generate SDP
        let mut inner = self.spawn_process(process_factory).await?;

        let evt = inner
            .sender
            .send(Command::RtpGenerateVideoSdp {
                display_id,
                yuv444,
                intra_refresh,
            })
            .await?;

        // Stop avserver
        Self::stop_inner(self.type_, inner).await?;

        // Unpack results
        let Some(Event::RtpSdpGenerated { sdp }) = evt else {
            return Err(Error::AvServerUnexpectedResponse);
        };

        Ok(sdp)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn rtp_start_video(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        ip: String,
        port: u16,
        bitrate: u32,
        yuv444: bool,
        intra_refresh: Option<bool>,
    ) -> Result<()> {
        self.start_inner(
            process_factory,
            Command::RtpStartVideo {
                display_id,
                ip,
                port,
                bitrate,
                yuv444,
                intra_refresh,
            },
        )
        .await?;

        Ok(())
    }

    pub(crate) async fn rtp_start_audio(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        ip: String,
        port: u16,
    ) -> Result<()> {
        self.start_inner(process_factory, Command::RtpStartAudio { ip, port })
            .await?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn kymux_start_video(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        uri: String,
        metrics_uri: Option<String>,
        bitrate: u32,
        codec: VideoCodec,
        yuv444: bool,
        intra_refresh: Option<bool>,
    ) -> Result<()> {
        self.start_inner(
            process_factory,
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
        .await?;

        Ok(())
    }

    pub(crate) async fn kymux_start_audio(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        uri: String,
    ) -> Result<()> {
        self.start_inner(process_factory, Command::KymuxStartAudio { uri })
            .await?;

        Ok(())
    }

    async fn start_inner(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        start_cmd: Command,
    ) -> Result<()> {
        let mut inner = self.spawn_process(process_factory).await?;
        inner.sender.send(start_cmd).await?;

        inner.start_receiver_task(
            self.type_,
            self.session_id,
            self.stream_id.clone(),
            self.statemachine_proxy.clone(),
        )?;
        self.inner = Some(inner);

        Ok(())
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
    async fn stop_inner(type_: HandlerType, mut inner: Inner) -> Result<()> {
        if let Some(receiver_task) = inner.receiver_task.take() {
            let _ = receiver_task.stop().await;
        }

        // A command send failure or a process that doesn't stop lead to a process kill
        inner.child.kill().await?;

        inner.commander.stop().await?;
        debug!("Commander {type_:?} stopped");

        Ok(())
    }

    pub(crate) async fn video_force_idr(&mut self) -> Result<()> {
        debug!("Requesting IDR frame");
        self.inner
            .as_mut()
            .ok_or(Error::StreamerStopped)?
            .sender
            .send_with_timeout(Command::VideoForceIdr, CMD_ACK_TIMEOUT)
            .await?;

        Ok(())
    }

    pub(crate) async fn video_set_bitrate(&mut self, bitrate: i32) -> Result<()> {
        self.inner
            .as_mut()
            .ok_or(Error::StreamerStopped)?
            .sender
            .send_with_timeout(Command::VideoSetBitrate { bitrate }, CMD_ACK_TIMEOUT)
            .await?;

        Ok(())
    }

    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(inner) = self.inner.take() else {
            return Err(Error::StreamerStopped);
        };

        Self::stop_inner(self.type_, inner).await?;

        info!("AvServer {type_:?} exited", type_ = self.type_);

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
    actor_join: Option<task::JoinHandle<()>>,
}

impl SharedHandler {
    /// Consume a started [`Handler`] and spawn its actor task.
    pub(crate) fn spawn(handler: Handler) -> Self {
        let type_ = handler.type_();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let actor_join = task::spawn(Self::actor_loop(handler, cmd_rx));
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
        match time::timeout(STOP_TIMEOUT, rx).await {
            Ok(Ok(res)) => {
                // Acked; the actor returns immediately after, so this is instant.
                let _ = join.await;
                res
            }
            Ok(Err(_)) => {
                // Actor dropped the ack without sending (panicked); reap it.
                let _ = join.await;
                Err(Error::StreamerStopped)
            }
            Err(_) => {
                warn!(
                    "AvServer {type_:?} stop timed out after {STOP_TIMEOUT:?}; aborting actor",
                    type_ = self.type_(),
                );
                join.abort();
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
                let res = if handler.is_running() {
                    handler.stop().await
                } else {
                    Ok(())
                };
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
        if handler.is_running() {
            if let Err(e) = handler.stop().await {
                warn!("Implicit avservice stop failed: {e:?}");
            }
        }
    }
}
