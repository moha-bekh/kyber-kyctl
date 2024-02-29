use std::sync::Arc;

use log::warn;
use tokio::{sync::oneshot, task};

use libkypc::{process::ProcessFactory, EventReceiver};

use crate::avservice;
use crate::{Error, Result, StateMachineEvent, StateMachineProxy};

pub(crate) struct Watchdog {
    avservice: avservice::Handler,
    avservice_handle: task::JoinHandle<()>,
    avservice_stop_tx: oneshot::Sender<()>,
}

impl Watchdog {
    pub(crate) async fn new(
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(Self, Vec<avservice_types::Display>)> {
        let mut avservice = avservice::Handler::new("DisplayEnum", statemachine_proxy.clone());

        let displays = avservice.enumerate_displays(process_factory).await?;

        let avservice_receiver = avservice.get_receiver().ok_or(Error::AvServerNoReceiver)?;

        let (avservice_stop_tx, avservice_stop_rx) = oneshot::channel();

        let avservice_handle = task::spawn(Self::avserver_task_entry(
            avservice_receiver,
            statemachine_proxy,
            avservice_stop_rx,
        ));

        Ok((
            Self {
                avservice,
                avservice_handle,
                avservice_stop_tx,
            },
            displays,
        ))
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        self.avservice.stop().await?;

        let ret = self.avservice_stop_tx.send(());
        if ret.is_err() {
            warn!("Avservice task stop send request failed");
        }

        self.avservice_handle.await.map_err(|err| {
            warn!("Watchdog task panicked: {err:?}");
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    async fn avserver_task_entry(
        receiver: EventReceiver<avservice_types::Event>,
        statemachine_proxy: StateMachineProxy,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::avserver_process_receiver(receiver, statemachine_proxy) => {}
        }
    }

    async fn avserver_process_receiver(
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
