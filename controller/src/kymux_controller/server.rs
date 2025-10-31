use std::sync::Arc;
use std::time::Duration;

use kymux::kyproto;
use log::{info, warn};
use tokio::sync::oneshot;
use tokio::task::{self, JoinHandle};

use crate::error::{Error, Result};

struct KymuxServerTaskRx {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,

    connected_rx: oneshot::Receiver<kyproto::Connection>,
}

pub(crate) struct KymuxServer {
    server: Arc<kymux::Server>,
    task_rx: Option<KymuxServerTaskRx>,
}

impl KymuxServer {
    pub(crate) async fn new(server: kymux::Server, auth: kyproto::ClientAuth) -> Result<Self> {
        let (stop_tx, stop_rx) = oneshot::channel();
        let (connected_tx, connected_rx) = oneshot::channel();

        let server = Arc::new(server);
        let server_task = server.clone();

        let join_handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                ret = Self::run_task(server_task, auth, connected_tx) => {
                    if let Err(err) = ret {
                        warn!("KymuxServer task ended with error {err:?}");
                    }
                }
            }
        });

        Ok(Self {
            server,
            task_rx: Some(KymuxServerTaskRx {
                stop_tx,
                join_handle,
                connected_rx,
            }),
        })
    }

    async fn stop_task_internal(
        stop_tx: oneshot::Sender<()>,
        join_handle: JoinHandle<()>,
    ) -> Result<()> {
        let ret = stop_tx.send(());
        if ret.is_err() {
            warn!("KymuxServer already stopped");
        }

        join_handle.await.map_err(|err| {
            warn!("KymuxServer panicked: {err:?}");
            Error::ThreadPanicked
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        if let Some(task_rx) = self.task_rx {
            Self::stop_task_internal(task_rx.stop_tx, task_rx.join_handle).await?;
        }

        self.server.close(0, "Server stopping");
        self.server.wait_idle().await;

        Ok(())
    }

    pub(crate) async fn wait_connected(
        &mut self,
        timeout: Duration,
    ) -> Result<Arc<kyproto::Connection>> {
        let Some(task_rx) = self.task_rx.take() else {
            warn!("Kymux not listening");
            return Err(Error::KymuxNotListening);
        };

        let connection = match tokio::time::timeout(timeout, task_rx.connected_rx).await {
            Ok(Ok(connection)) => Ok(Arc::new(connection)),
            Ok(Err(_)) => Err(Error::ChannelClosedRx),
            Err(_) => {
                warn!("Got not Kymux connection after {timeout:?}");
                Err(Error::KymuxAcceptTimeout)
            }
        };

        let ret = Self::stop_task_internal(task_rx.stop_tx, task_rx.join_handle).await;
        if let Err(err) = ret {
            warn!("Fail to stop KymuxServer: {err:?}");
        }

        connection
    }

    async fn run_task(
        server: Arc<kymux::Server>,
        auth: kyproto::ClientAuth,
        connected_tx: oneshot::Sender<kyproto::Connection>,
    ) -> Result<()> {
        info!("Waiting for Kymux connection");

        loop {
            let connection = server.accept_with_auth().await?;

            let connection_auth = connection.get_auth();
            if *connection_auth != auth {
                warn!("Got invalid connection. Reject it");
                connection.reject_authentication();
                continue;
            }

            let connection = match connection.accept_authentication().await {
                Ok(connection) => connection,
                Err(err) => {
                    warn!("Accept authentication failed: {err:?}");
                    continue;
                }
            };

            let ret = connected_tx.send(connection);
            if ret.is_err() {
                warn!("Fail to send Kymux connection notification");
            }

            return Ok(());
        }
    }
}
