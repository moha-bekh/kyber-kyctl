use actix_web::{web, HttpRequest, HttpResponse};
use log::warn;
use serde::Serialize;
use tokio::{
    sync::{mpsc, oneshot},
    task,
};

use crate::{Error, Result, StateMachineProxy};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Display {
    pub id: u32,
    pub name: String,
    pub height: i32,
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Serialize)]
pub(crate) enum HostEvent {
    DisplayList { displays: Vec<Display> },
}

#[derive(Debug, Serialize)]
enum HostEventFrame {
    Event { event: HostEvent },
}

pub(crate) struct HostEventSender {
    events_tx: mpsc::Sender<HostEventFrame>,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl HostEventSender {
    pub(crate) fn new(
        req: HttpRequest,
        stream: web::Payload,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(Self, HttpResponse)> {
        let (res, session, msg_stream) = actix_ws::handle(&req, stream)?;

        let (events_tx, events_rx) = mpsc::channel(32);
        let (stop_tx, stop_rx) = oneshot::channel();

        let handle = task::spawn_local(Self::task_entry(
            session,
            msg_stream,
            events_tx.clone(),
            events_rx,
            stop_rx,
            statemachine_proxy,
        ));

        Ok((
            Self {
                events_tx,
                handle,
                stop_tx,
            },
            res,
        ))
    }

    pub(crate) async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if let Err(err) = ret {
            warn!("Failed to send stop event to task: {err:?}");
        }

        self.handle.await.map_err(|err| {
            warn!("HostEvent task panicked: {err:?}");
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    pub(crate) async fn send(&self, event: HostEvent) {
        let ret = self.events_tx.send(HostEventFrame::Event { event }).await;
        if let Err(err) = ret {
            warn!("Failed to send event {err:?}");
        }
    }

    async fn task_entry(
        session: actix_ws::Session,
        _msg_stream: actix_ws::MessageStream,
        _events_tx: mpsc::Sender<HostEventFrame>,
        events_rx: mpsc::Receiver<HostEventFrame>,
        stop_rx: oneshot::Receiver<()>,
        _statemachine_proxy: StateMachineProxy,
    ) {
        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_events(session, _msg_stream, events_rx) => {}
        }
    }

    async fn process_events(
        mut session: actix_ws::Session,
        mut _msg_stream: actix_ws::MessageStream,
        mut events_rx: mpsc::Receiver<HostEventFrame>,
    ) {
        while let Some(msg) = events_rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(json) => json,
                Err(err) => {
                    warn!("Fail to convert message to JSON {msg:?}: {err:?}");
                    continue;
                }
            };

            let ret = session.text(json).await;
            if let Err(err) = ret {
                warn!("Websocket send failed: {err:?}");
                break;
            }
        }
    }
}
