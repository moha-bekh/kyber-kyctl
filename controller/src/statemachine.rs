use actix_web::web;
use log::{info, warn};
use tokio::{
    sync::{mpsc, oneshot, Mutex},
    task,
};

use crate::utils::TypeConvert;
use crate::{avservice, host_events};
use crate::{Controller, Error, HostEvent, Result};

#[derive(Debug)]
pub(crate) enum StateMachineEvent {
    AvService {
        event: avservice_types::Event,
    },
    AvServiceStopped {
        type_: avservice::HandlerType,
    },
    HostEvents {
        notification: host_events::Notification,
    },
}

#[derive(Clone)]
pub(crate) struct StateMachineProxy {
    events_tx: mpsc::Sender<StateMachineEvent>,
}

impl StateMachineProxy {
    pub(crate) async fn send(&self, msg: StateMachineEvent) {
        let ret = self.events_tx.send(msg).await;
        if let Err(err) = ret {
            warn!("Failed to send event {err:?}");
        }
    }
}

pub(crate) struct StateMachine {
    events_tx: mpsc::Sender<StateMachineEvent>,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl StateMachine {
    pub(crate) fn new(ctrl: web::Data<Mutex<Controller>>) -> Result<Self> {
        let (events_tx, events_rx) = mpsc::channel(32);
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn_local(Self::task_entry(ctrl, events_rx, stop_rx));

        Ok(Self {
            events_tx,
            handle,
            stop_tx,
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("Statemachine task stop send request failed");
        }

        self.handle.await.map_err(|err| {
            warn!("Statemachine task panicked: {err:?}");
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    pub(crate) fn create_proxy(&self) -> StateMachineProxy {
        StateMachineProxy {
            events_tx: self.events_tx.clone(),
        }
    }

    async fn task_entry(
        ctrl: web::Data<Mutex<Controller>>,
        events_rx: mpsc::Receiver<StateMachineEvent>,
        stop_rx: oneshot::Receiver<()>,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_events(ctrl, events_rx) => {}
        }
    }

    #[allow(clippy::single_match)]
    async fn process_events(
        ctrl: web::Data<Mutex<Controller>>,
        mut events_rx: mpsc::Receiver<StateMachineEvent>,
    ) {
        while let Some(evt) = events_rx.recv().await {
            match evt {
                StateMachineEvent::AvService {
                    event: avservice_types::Event::DisplayListUpdated { displays },
                } => {
                    info!("Got display update: {displays:?}");
                    let mut ctrl = ctrl.lock().await;

                    if let Some(client_session) = ctrl.client_session.as_ref() {
                        client_session
                            .host_event_sender
                            .send(HostEvent::DisplayList {
                                displays: displays.convert(),
                            })
                            .await;
                    };

                    if let Some(inputservice) = ctrl.services.input.as_mut() {
                        let ret = inputservice
                            .update_host_config(inputservice_types::HostConfig {
                                display_list: displays.convert(),
                            })
                            .await;
                        if let Err(err) = ret {
                            warn!("Failed to send HostConfig to InputServer: {err:?}");
                        }
                    }

                    ctrl.displays = displays;
                }
                StateMachineEvent::AvService {
                    event: avservice_types::Event::PipelineEvent(event),
                } => {
                    let ctrl = ctrl.lock().await;

                    if let Some(client_session) = ctrl.client_session.as_ref() {
                        client_session
                            .host_event_sender
                            .send(HostEvent::StreamerPipelineEvent(event.into()))
                            .await;
                    };
                }
                StateMachineEvent::AvServiceStopped { type_ } => {
                    info!("AvServer {type_:?} stopped");
                }
                StateMachineEvent::HostEvents { notification } => match notification {
                    host_events::Notification::ConnectionLost => {
                        warn!("Control Websocket disconnected");

                        let mut ctrl = ctrl.lock().await;

                        let ret = ctrl.stop().await;
                        if let Err(err) = ret {
                            warn!("Fail to stop Controller: {err:?}");
                        }
                    }
                },
                _ => {}
            }
        }
    }
}
