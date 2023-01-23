use std::path::PathBuf;

use log::{debug, info};

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

    async fn spawn_process(&self, process_factory: &mut ProcessFactory) -> Result<Inner> {
        info!("Run AvServer {name}", name = self.name);

        let (child, mut commander): (_, Commander<Command, Event>) =
            process_factory.spawn(&self.avserver_path, None).await?;
        let (sender, receiver) = master.get_sender_receiver()?;

        Ok(Inner {
            child,
            commander,
            sender,
            _receiver: receiver,
        })
    }

    pub(crate) async fn start(
        &mut self,
        process_factory: &mut ProcessFactory,
        script: &str,
        args: Option<Vec<String>>,
    ) -> Result<()> {
        if self.inner.is_some() {
            return Err(Error::StreamerStarted);
        }

        let script_path = utils::get_resource_path(script);

        let mut inner = self.spawn_process(process_factory).await?;
        inner
            .sender
            .send(Command::Start {
                script: utils::path_to_string(&script_path)?,
                args,
            })
            .await?;
        self.inner = Some(inner);

        Ok(())
    }

    pub(crate) async fn stop(&mut self) -> Result<()> {
        let Some(mut inner) = self.inner.take() else {
            return Err(Error::StreamerStopped);
        };

        inner.sender.send(Command::Stop).await?;

        debug!("Stop child");
        inner.child.stop().await?;
        debug!("Stop child: Done");

        debug!("Stop commander");
        inner.commander.stop().await?;
        debug!("Commander {name} stopped", name = self.name);

        info!("AvServer exited");

        Ok(())
    }
}
