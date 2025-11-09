//! Background task for enforcing session expiration.
//!
//! This module implements a periodic cleanup task that terminates expired sessions
//! every 30 seconds. It complements the request-time expiration checks in the
//! Identity middleware.
//!
//! ## Why both background task and middleware?
//!
//! - **Middleware**: Validates expiration on HTTP requests
//! - **Background task**: Handles WebSocket-only connections that don't make HTTP requests
//!
//! Without the background task, a user with only a WebSocket connection could continue
//! streaming indefinitely even after their session expires, since no HTTP requests would
//! trigger the middleware validation.
//!
//! ## Cleanup Process
//!
//! Every 30 seconds, the task:
//! 1. Checks if a client session exists
//! 2. Checks if the session has an expiration time
//! 3. Compares expiration to current UTC time
//! 4. If expired: Calls `Controller::stop()` to gracefully terminate streaming services

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
    /// Creates and starts a new expiration monitoring task.
    ///
    /// The task spawns immediately and begins checking for expired sessions
    /// every 30 seconds.
    ///
    /// # Parameters
    /// - `ctrl`: Shared reference to the Controller
    ///
    /// # Returns
    /// An `ExpirationTask` handle that can be used to stop the task
    pub(crate) fn new(ctrl: web::Data<Mutex<Controller>>) -> Self {
        let stop = Arc::new(Notify::new());
        let join_handle = tokio::spawn(Self::task_entry(ctrl, stop.clone()));

        Self { stop, join_handle }
    }

    /// Gracefully stops the expiration monitoring task.
    ///
    /// Signals the task to exit and waits for it to complete. Should be called
    /// during application shutdown.
    pub(crate) async fn stop(self) {
        self.stop.notify_one();
        let err = self.join_handle.await;
        if let Err(err) = err {
            warn!("Session expiration watcher failed to join: {err}");
        }
    }

    /// Main task loop - runs continuously until stopped.
    async fn task_entry(ctrl: web::Data<Mutex<Controller>>, stop: Arc<Notify>) {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = stop.notified() => break,
                _ = interval.tick() => Self::check_expiration(&ctrl).await,
            }
        }
    }

    /// Checks if the current session has expired and terminates it if so.
    ///
    /// ## Process:
    ///
    /// 1. Lock the controller (brief - no I/O while locked)
    /// 2. Check if a client session exists (early return if none)
    /// 3. Check if session has expiration time (early return for Basic auth)
    /// 4. Compare expiration to current UTC time
    /// 5. If expired: Call `Controller::stop()` to terminate all streaming services
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
