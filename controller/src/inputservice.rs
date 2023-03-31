use std::sync::Arc;

use log::{debug, info, warn};

use inputservice_types::{Command, Event, HostConfig};
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
    inner: Option<Inner>,
}

impl Handler {
    pub(crate) fn new() -> Self {
        Self { inner: None }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.inner.is_some()
    }

    async fn spawn_process(process_factory: &Arc<ProcessFactory>) -> Result<Inner> {
        debug!("Run Input Server");

        let exe_path = utils::get_executable_path("inputserver");

        let (child, mut commander): (_, Commander<Command, Event>) =
            process_factory.spawn(exe_path, None).await?;
        let (sender, receiver) = commander.get_sender_receiver()?;

        Ok(Inner {
            child,
            commander,
            sender,
            _receiver: receiver,
        })
    }

    pub(crate) async fn start_tcp(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        host_config: HostConfig,
    ) -> Result<()> {
        if self.inner.is_some() {
            return Err(Error::StreamerStarted);
        }

        let mut inner = Self::spawn_process(process_factory).await?;
        inner.sender.send(Command::StartTcp { host_config }).await?;
        self.inner = Some(inner);

        Ok(())
    }

    pub(crate) async fn start_kymux(
        &mut self,
        process_factory: &Arc<ProcessFactory>,
        host_config: HostConfig,
        uri: String,
    ) -> Result<()> {
        if self.inner.is_some() {
            return Err(Error::StreamerStarted);
        }

        let mut inner = Self::spawn_process(process_factory).await?;
        inner
            .sender
            .send(Command::StartKymux { host_config, uri })
            .await?;
        self.inner = Some(inner);

        Ok(())
    }

    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(mut inner) = self.inner.take() else {
            return Err(Error::StreamerStopped);
        };

        let ret = inner.sender.send(Command::Stop(0)).await;
        if let Err(err) = ret {
            warn!("Stop request send failed: {err:?}");
        }

        debug!("Stop child");
        inner.child.stop().await?;
        debug!("Stop child: Done");

        debug!("Stop commander");
        inner.commander.stop().await?;
        debug!("Stop commander: Done");

        info!("Input server exited");

        Ok(())
    }
}
