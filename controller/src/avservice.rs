use std::path::PathBuf;

use log::{debug, info, warn};

use avservice_types::{Command, Event};
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
    _receiver: EventReceiver<Event>,
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

    async fn spawn_process(&self, process_factory: &mut ProcessFactory) -> Result<Inner> {
        info!("Run AvServer {name}", name = self.name);

        let (child, mut commander): (_, Commander<Command, Event>) =
            process_factory.spawn(&self.avserver_path, None).await?;
        let (sender, receiver) = commander.get_sender_receiver()?;

        Ok(Inner {
            child,
            commander,
            sender,
            _receiver: receiver,
        })
    }

    pub(crate) async fn rtp_generate_video_sdp(
        &self,
        process_factory: &mut ProcessFactory,
        yuv444: bool,
    ) -> Result<String> {
        // Generate SDP
        let mut inner = self.spawn_process(process_factory).await?;

        let evt = inner
            .sender
            .send(Command::RtpGenerateVideoSdp { yuv444 })
            .await?;

        // Stop avserver
        self.stop_inner(inner).await?;

        // Unpack results
        let Some(Event::RtpSdpGenerated { sdp }) = evt else {
            return Err(Error::AvServerUnexpectedResponse);
        };

        Ok(sdp)
    }

    pub(crate) async fn rtp_start_video(
        &mut self,
        process_factory: &mut ProcessFactory,
        ip: String,
        port: u16,
        yuv444: bool,
    ) -> Result<()> {
        let mut inner = self.spawn_process(process_factory).await?;

        inner
            .sender
            .send(Command::RtpStartVideo { ip, port, yuv444 })
            .await?;

        self.inner = Some(inner);

        Ok(())
    }

    pub(crate) async fn rtp_start_audio(
        &mut self,
        process_factory: &mut ProcessFactory,
        ip: String,
        port: u16,
    ) -> Result<()> {
        let mut inner = self.spawn_process(process_factory).await?;

        inner
            .sender
            .send(Command::RtpStartAudio { ip, port })
            .await?;

        self.inner = Some(inner);

        Ok(())
    }

    async fn stop_inner(&self, mut inner: Inner) -> Result<()> {
        debug!("Send stop request to {name}", name = self.name);
        let ret = inner.sender.send(Command::Stop).await;
        if let Err(err) = ret {
            warn!("Stop request send failed: {err:?}");
        }

        inner.child.stop().await?;
        debug!("Child {name} stopped", name = self.name);

        inner.commander.stop().await?;
        debug!("Commander {name} stopped", name = self.name);

        Ok(())
    }

    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(inner) = self.inner.take() else {
            return Err(Error::StreamerStopped);
        };

        self.stop_inner(inner).await?;

        info!("AvServer {name} exited", name = self.name);

        Ok(())
    }
}
