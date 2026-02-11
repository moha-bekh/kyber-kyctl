// Project Kyber: avservice.rs
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

use std::path::PathBuf;
use std::sync::Arc;

use log::{debug, info, warn};
use tokio::{sync::oneshot, task};

use kyavservice_types::{Command, Display, Event, VideoCodec};
use libkypc::{
    process::{Process, ProcessFactory},
    CommandSender, Commander, EventReceiver,
};

use crate::utils;
use crate::{Error, Result, StateMachineEvent, StateMachineProxy};

struct ReceiverTask {
    type_: HandlerType,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl ReceiverTask {
    fn new(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        receiver: EventReceiver<Event>,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(
            type_,
            session_id,
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
        receiver: EventReceiver<kyavservice_types::Event>,
        statemachine_proxy: StateMachineProxy,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_receiver(type_, session_id, receiver, statemachine_proxy) => {}
        }
    }

    async fn process_receiver(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        mut receiver: EventReceiver<kyavservice_types::Event>,
        statemachine_proxy: StateMachineProxy,
    ) {
        while let Ok(event) = receiver.recv().await {
            statemachine_proxy
                .send(StateMachineEvent::AvService { session_id, event })
                .await;
        }

        statemachine_proxy
            .send(StateMachineEvent::AvServiceStopped { session_id, type_ })
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
        statemachine_proxy: StateMachineProxy,
    ) -> Result<()> {
        let receiver = self.receiver.take().ok_or(Error::AvServerNoReceiver)?;
        self.receiver_task = Some(ReceiverTask::new(
            type_,
            session_id,
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
    avserver_path: PathBuf,
    statemachine_proxy: StateMachineProxy,
    inner: Option<Inner>,
}

impl Handler {
    pub(crate) fn new(
        type_: HandlerType,
        session_id: Option<uuid::Uuid>,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        Self {
            type_,
            session_id,
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

        inner.start_receiver_task(self.type_, self.session_id, self.statemachine_proxy.clone())?;
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

        inner.start_receiver_task(self.type_, self.session_id, self.statemachine_proxy.clone())?;
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

    pub(crate) async fn video_set_bitrate(&mut self, bitrate: i32) -> Result<()> {
        self.inner
            .as_mut()
            .ok_or(Error::StreamerStopped)?
            .sender
            .send(Command::VideoSetBitrate { bitrate })
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
