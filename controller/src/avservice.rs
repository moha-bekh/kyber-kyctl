use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};

use avservice_types::{Command, Display, Event, VideoCodec};
use libkypc::{
    process::{Process, ProcessFactory},
    CommandSender, Commander, EventReceiver,
};

use crate::error::{Error, Result};
use crate::utils;

struct Inner {
    child: Process,
    commander: Commander<Command, Event>,
    sender: CommandSender<Command, Event>,
    receiver: Option<EventReceiver<Event>>,
}

pub(crate) struct Handler {
    name: String,
    avserver_path: PathBuf,
    inner: Option<Inner>,
}

impl Handler {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            avserver_path: utils::get_executable_path("avserver"),
            inner: None,
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    pub(crate) fn name(&self) -> String {
        self.name.clone()
    }

    pub(crate) fn get_receiver(&mut self) -> Option<EventReceiver<Event>> {
        let Some(inner) = self.inner.as_mut() else {
            return None;
        };

        inner.receiver.take()
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
