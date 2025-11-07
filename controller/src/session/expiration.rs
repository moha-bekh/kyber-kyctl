use std::sync::Arc;
use std::time::Duration;

use actix_web::web;
use log::{error, warn};
use time::OffsetDateTime;
use tokio::sync::{Mutex, Notify};

use crate::Controller;

pub(crate) struct ExpirationTask {
    stop: Arc<Notify>,
    join_handle: tokio::task::JoinHandle<()>,
}

impl ExpirationTask {
    pub(crate) fn new(ctrl: web::Data<Mutex<Controller>>) -> Self {
        let stop = Arc::new(Notify::new());
        let join_handle = tokio::spawn(Self::task_entry(ctrl, stop.clone()));

        Self { stop, join_handle }
    }

    pub(crate) async fn stop(self) {
        self.stop.notify_one();
        let err = self.join_handle.await;
        if let Err(err) = err {
            warn!("Session expiration watcher failed to join: {err}");
        }
    }

    async fn task_entry(ctrl: web::Data<Mutex<Controller>>, stop: Arc<Notify>) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = stop.notified() => break,
                _ = interval.tick() => Self::check_expiration(&ctrl).await,
            }
        }
    }

    async fn check_expiration(ctrl: &web::Data<Mutex<Controller>>) {
        let mut ctrl = ctrl.lock().await;

        let Some(session) = ctrl.client_session.as_ref() else {
            return;
        };
        let Some(exp) = session.expiration else {
            return;
        };

        if OffsetDateTime::now_utc() > exp {
            warn!("Killing expired session \"{}\"", session.uid);
            let ret = ctrl.stop().await;
            if let Err(err) = ret {
                error!("Fail to stop session: {err:?}");
            }
        }
    }
}
