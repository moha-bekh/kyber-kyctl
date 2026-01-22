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
//! 1. Iterates through all client sessions
//! 2. Checks if each session has an expiration time (skipped for Basic auth)
//! 3. Compares expiration to current UTC time
//! 4. If expired: Calls `Controller::stop()` to terminate that session's streaming services
//! 5. Cleans up expired pending authentication tokens

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

    /// Checks if any sessions have expired and terminates them.
    ///
    /// ## Process:
    ///
    /// 1. Lock the controller (brief - no I/O while locked)
    /// 2. Iterate through all client sessions
    /// 3. Check if session has expiration time (skip for Basic auth)
    /// 4. Compare expiration to current UTC time
    /// 5. If expired: Call `Controller::stop()` to terminate that session's streaming services
    /// 6. Clean up any expired pending authentication tokens
    async fn check_expiration(ctrl: &web::Data<Mutex<Controller>>) {
        let mut ctrl = ctrl.lock().await;

        let now = OffsetDateTime::now_utc();
        let expired_sessions: Vec<_> = ctrl
            .sessions
            .iter()
            .filter_map(|(uid, session)| {
                session
                    .expiration
                    .and_then(|exp| if now > exp { Some(*uid) } else { None })
            })
            .collect();

        for session_id in expired_sessions {
            warn!("Killing expired session \"{}\"", session_id);
            let ret = ctrl.stop(session_id).await;
            if let Err(err) = ret {
                error!("Fail to stop session: {err:?}");
            }
        }

        // Cleanup expired pending auth tokens in shared Kymux infrastructure
        ctrl.shared_kymux_infra.lock().await.cleanup_expired().await;
    }
}
