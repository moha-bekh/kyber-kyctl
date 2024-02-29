use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};
use tokio::{sync::oneshot, task};

use avservice_types::{Command, Display, Event, VideoCodec};
use libkypc::{
    process::{Process, ProcessFactory},
    CommandSender, Commander, EventReceiver,
};

use crate::utils;
use crate::{Error, Result, StateMachineEvent, StateMachineProxy};

struct ReceiverTask {
    name: String,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl ReceiverTask {
    fn new(
        name: String,
        receiver: EventReceiver<Event>,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn(Self::task_entry(receiver, statemachine_proxy, stop_rx));

        Self {
            name,
            handle,
            stop_tx,
        }
    }

    async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("{} receiver task stop send request failed", self.name);
        }

        self.handle.await.map_err(|err| {
            warn!("{} receiver task panicked: {err:?}", self.name);
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    async fn task_entry(
        receiver: EventReceiver<avservice_types::Event>,
        statemachine_proxy: StateMachineProxy,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_receiver(receiver, statemachine_proxy) => {}
        }
    }

    async fn process_receiver(
        mut receiver: EventReceiver<avservice_types::Event>,
        statemachine_proxy: StateMachineProxy,
    ) {
        while let Ok(event) = receiver.recv().await {
            statemachine_proxy
                .send(StateMachineEvent::AvService { event })
                .await;
        }
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
        name: String,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<()> {
        let receiver = self.receiver.take().ok_or(Error::AvServerNoReceiver)?;
        self.receiver_task = Some(ReceiverTask::new(name, receiver, statemachine_proxy));

        Ok(())
    }
}

pub(crate) struct Handler {
    name: String,
    avserver_path: PathBuf,
    statemachine_proxy: StateMachineProxy,
    inner: Option<Inner>,
}

impl Handler {
    pub(crate) fn new(name: &str, statemachine_proxy: StateMachineProxy) -> Self {
        Self {
            name: name.into(),
            avserver_path: utils::get_executable_path("avserver"),
            inner: None,
            statemachine_proxy,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    pub(crate) fn name(&self) -> String {
        self.name.clone()
    }

    async fn spawn_process(&self, process_factory: &Arc<ProcessFactory>) -> Result<Inner> {
        debug!("Run AvServer {name}", name = self.name);

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

        inner.start_receiver_task(self.name.clone(), self.statemachine_proxy.clone())?;
        self.inner = Some(inner);

        Ok(displays)
    }

    pub(crate) async fn rtp_generate_video_sdp(
        &self,
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        yuv444: bool,
    ) -> Result<String> {
        // Generate SDP
        let mut inner = self.spawn_process(process_factory).await?;

        let evt = inner
            .sender
            .send(Command::RtpGenerateVideoSdp { display_id, yuv444 })
            .await?;

        // Stop avserver
        Self::stop_inner(&self.name, inner).await?;

        // Unpack results
        let Some(Event::RtpSdpGenerated { sdp }) = evt else {
            return Err(Error::AvServerUnexpectedResponse);
        };

        Ok(sdp)
    }

    pub(crate) async fn rtp_start_video(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        ip: String,
        port: u16,
        bitrate: u32,
        yuv444: bool,
    ) -> Result<()> {
        self.start_inner(
            process_factory,
            Command::RtpStartVideo {
                display_id,
                ip,
                port,
                bitrate,
                yuv444,
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

    pub(crate) async fn kymux_start_video(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        display_id: u32,
        uri: String,
        bitrate: u32,
        codec: VideoCodec,
        yuv444: bool,
    ) -> Result<()> {
        self.start_inner(
            process_factory,
            Command::KymuxStartVideo {
                display_id,
                uri,
                bitrate,
                codec,
                yuv444,
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

        inner.start_receiver_task(self.name.clone(), self.statemachine_proxy.clone())?;
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
    async fn stop_inner(name: &str, mut inner: Inner) -> Result<()> {
        if let Some(receiver_task) = inner.receiver_task.take() {
            let _ = receiver_task.stop().await;
        }

        const STOP_TIMEOUT: Duration = Duration::from_secs(3);

        info!("Send stop request to {name}");
        let wait_process = match inner
            .sender
            .send_with_timeout(Command::Stop, STOP_TIMEOUT)
            .await
        {
            Ok(_) => true,
            Err(err) => {
                warn!("Stop request send failed: {err:?}");
                false
            }
        };

        // At first, wait for the process if requested
        let kill_process = if wait_process {
            match tokio::time::timeout(STOP_TIMEOUT, inner.child.stop()).await {
                Ok(_) => false,
                Err(_) => {
                    warn!("{name} didn't stop after {STOP_TIMEOUT:?}",);
                    true
                }
            }
        } else {
            true
        };

        // A command send failure or a process that doesn't stop lead to a process kill
        if kill_process {
            inner.child.kill().await?;
        }

        inner.commander.stop().await?;
        debug!("Commander {name} stopped");

        Ok(())
    }

    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(inner) = self.inner.take() else {
            return Err(Error::StreamerStopped);
        };

        Self::stop_inner(&self.name, inner).await?;

        info!("AvServer {name} exited", name = self.name);

        Ok(())
    }
}
