use std::time::Duration;

use actix_ws::{CloseCode, CloseReason};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{mpsc, oneshot},
    task,
};

use crate::{Error, Result, StateMachineEvent, StateMachineProxy};

// Naming taken from man(7) tcp: TCP_KEEPCNT/TCP_KEEPINTVL
pub(crate) const KEEPINTVL: Duration = Duration::from_secs(20);
pub(crate) const KEEPCNT: u64 = 3;

struct PingTask {
    ping_rx: mpsc::Receiver<u64>,
    events_tx: mpsc::Sender<HostEventFrame>,
    statemachine_proxy: StateMachineProxy,
}

impl PingTask {
    async fn run(&mut self) {
        let mut interval = tokio::time::interval(KEEPINTVL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        const FIRST_ID: u64 = 0;

        let mut last_id_received = None;
        let mut next_id = FIRST_ID;

        loop {
            tokio::select! {
                biased;
                // Send Ping
                _ = interval.tick() => {
                    // Prepare id
                    let id = next_id;
                    next_id += 1;

                    // Check if timeout occurred
                    // - If true: Notify of disconnection lost
                    // - Else continue to ping
                    if id - last_id_received.unwrap_or(FIRST_ID) >= KEEPCNT {
                        self.statemachine_proxy.send(StateMachineEvent::HostEvents { notification: Notification::ConnectionLost })
                        .await;

                        break;
                    } else {
                        let msg = HostEventFrame::Ping { id };

                        let ret = self.events_tx.send(msg).await;
                        if let Err(err) = ret {
                            warn!("Failed to Ping({id}): {err:?}");
                            break;
                        }
                    }
                }
                // Pong received
                id = self.ping_rx.recv() => {
                    let Some(id) = id else {
                        warn!("Ping Rx channel closed");
                        break;
                    };

                    last_id_received = Some(id);
                }
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum Notification {
    ConnectionLost,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Display {
    pub id: u32,
    pub name: String,
    pub height: i32,
    pub width: i32,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

impl From<avservice_types::PipelineType> for StreamerPipelineType {
    fn from(t: avservice_types::PipelineType) -> Self {
        match t {
            avservice_types::PipelineType::Audio => Self::Audio,
            avservice_types::PipelineType::Video => Self::Video,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

impl From<avservice_types::PipelineStage> for StreamerPipelineStage {
    fn from(t: avservice_types::PipelineStage) -> Self {
        match t {
            avservice_types::PipelineStage::Initialization => Self::Initialization,
            avservice_types::PipelineStage::Acquisition => Self::Acquisition,
            avservice_types::PipelineStage::Encoder => Self::Encoder,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum StreamerPipelineEventType {
    Stopped,
    Error,
}

impl From<avservice_types::PipelineEventType> for StreamerPipelineEventType {
    fn from(event: avservice_types::PipelineEventType) -> Self {
        match event {
            avservice_types::PipelineEventType::Error => Self::Error,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StreamerPipelineEvent {
    pub type_: StreamerPipelineType,
    pub stage: StreamerPipelineStage,
    pub event: StreamerPipelineEventType,
}

impl From<avservice_types::PipelineEvent> for StreamerPipelineEvent {
    fn from(event: avservice_types::PipelineEvent) -> Self {
        Self {
            type_: event.type_.into(),
            stage: event.stage.into(),
            event: event.event.into(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) enum HostEvent {
    DisplayList { displays: Vec<Display> },
    StreamerPipelineEvent(StreamerPipelineEvent),
    BitrateChanged { bitrate: u32 },
}

#[derive(Debug, Deserialize, Serialize)]
enum HostAuthEvent {
    Request,
    Token(String),
    Succeed,
}

#[derive(Debug, Deserialize, Serialize)]
enum HostEventFrame {
    Ping { id: u64 },
    Pong { id: u64 },
    Event { event: HostEvent },
}

pub(crate) struct HostEventSender {
    events_tx: mpsc::Sender<HostEventFrame>,
    handle: task::JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl HostEventSender {
    fn new(
        session: actix_ws::Session,
        msg_stream: actix_ws::MessageStream,
        statemachine_proxy: StateMachineProxy,
    ) -> Self {
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

        Self {
            events_tx,
            handle,
            stop_tx,
        }
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
        msg_stream: actix_ws::MessageStream,
        events_tx: mpsc::Sender<HostEventFrame>,
        events_rx: mpsc::Receiver<HostEventFrame>,
        stop_rx: oneshot::Receiver<()>,
        statemachine_proxy: StateMachineProxy,
    ) {
        let (ping_task_tx, ping_task_rx) = mpsc::channel(32);

        let mut ping_task = PingTask {
            ping_rx: ping_task_rx,
            events_tx,
            statemachine_proxy: statemachine_proxy.clone(),
        };

        tokio::select! {
            _ = stop_rx => {
            }
            _ = Self::process_rx_events(msg_stream, ping_task_tx, statemachine_proxy.clone()) => {}
            _ = Self::process_tx_events(session.clone(), events_rx, statemachine_proxy) => {}
            _ = ping_task.run() => {}
        }

        let _ = session.close(None).await;
    }

    async fn process_rx_events(
        mut stream: actix_ws::MessageStream,
        ping_task_tx: mpsc::Sender<u64>,
        statemachine_proxy: StateMachineProxy,
    ) {
        loop {
            let msg = match stream.recv().await {
                None | Some(Ok(actix_ws::Message::Close(_))) => {
                    info!("End of stream");
                    break;
                }
                Some(Ok(actix_ws::Message::Text(msg))) => msg,
                Some(Ok(_)) => {
                    continue;
                }
                Some(Err(err)) => {
                    warn!("recv() failed: {err:?}");
                    break;
                }
            };

            match serde_json::from_str(&msg) {
                Ok(HostEventFrame::Ping { id }) => {
                    warn!("Unexpected Ping({id})");
                }
                Ok(HostEventFrame::Pong { id }) => {
                    let ret = ping_task_tx.send(id).await;
                    if ret.is_err() {
                        warn!("Failed to send ping to Ping task");
                    }
                }
                Ok(HostEventFrame::Event { event }) => {
                    info!("Got event: {event:?}");
                }
                Err(err) => {
                    error!("Fail to parse {msg}: {err:?}");
                }
            }
        }

        statemachine_proxy
            .send(StateMachineEvent::HostEvents {
                notification: Notification::ConnectionLost,
            })
            .await;
    }

    async fn process_tx_events(
        mut session: actix_ws::Session,
        mut events_rx: mpsc::Receiver<HostEventFrame>,
        statemachine_proxy: StateMachineProxy,
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

                statemachine_proxy
                    .send(StateMachineEvent::HostEvents {
                        notification: Notification::ConnectionLost,
                    })
                    .await;

                break;
            }
        }
    }
}

pub(crate) async fn wait_authentication(
    session: actix_ws::Session,
    msg_stream: actix_ws::MessageStream,
) -> Result<(String, PendingHostEventSender)> {
    let mut pending_sender = PendingHostEventSender {
        session,
        msg_stream,
    };
    let Ok(resp) =
        tokio::time::timeout(Duration::from_secs(5), pending_sender.msg_stream.recv()).await
    else {
        pending_sender.reject().await;
        return Err(Error::WsAuthTimeout);
    };

    let msg = match resp {
        None | Some(Ok(actix_ws::Message::Close(_))) => {
            pending_sender.reject().await;
            return Err(Error::ChannelClosedRx);
        }
        Some(Ok(actix_ws::Message::Text(msg))) => msg,
        _ => {
            pending_sender.reject().await;
            return Err(Error::WsInvalidAuthMessage);
        }
    };

    #[derive(Deserialize)]
    struct Token {
        token: String,
    }
    let Ok(token_holder) = serde_json::from_str::<Token>(&msg) else {
        pending_sender.reject().await;
        return Err(Error::WsInvalidAuthMessage);
    };

    Ok((token_holder.token, pending_sender))
}

pub(crate) struct PendingHostEventSender {
    session: actix_ws::Session,
    msg_stream: actix_ws::MessageStream,
}

impl PendingHostEventSender {
    pub(crate) async fn accept(mut self, proxy: StateMachineProxy) -> Result<HostEventSender> {
        const SUCCESS_JSON: &str = "\"Welcome\"";
        let Ok(_) = self.session.text(SUCCESS_JSON).await else {
            self.reject().await;
            return Err(Error::ChannelClosedTx);
        };

        Ok(HostEventSender::new(self.session, self.msg_stream, proxy))
    }

    pub(crate) async fn reject(self) {
        let reason = CloseReason {
            code: CloseCode::Protocol,
            description: Some("Invalid credentials".to_owned()),
        };
        let _ = self.session.close(Some(reason)).await;
    }
}
