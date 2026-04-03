// Project Kyber: server.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kymux::{kynet, kyproto};
use log::{error, info, warn};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{self, JoinHandle};
use uuid::Uuid;

use crate::error::{Error, Result};

/// Timeout for pending authentication tokens (tokens not used within this duration are cleaned up)
const PENDING_AUTH_TIMEOUT: Duration = Duration::from_secs(60);

fn is_token_expired(created_at: &Instant) -> bool {
    created_at.elapsed() > PENDING_AUTH_TIMEOUT
}

/// Routes incoming Kymux connections to the appropriate session based on authentication token.
///
/// Flow:
/// 1. start_mux: adds pending_auth[token] = session_id AND session_channels[session_id] = Sender
/// 2. Client connects with token
/// 3. Server: validates token, sends connection via session_channels[session_id]
/// 4. start_session: awaits on the Receiver (stored in KymuxBackend)
pub(crate) struct IncomingConnectionRouter {
    /// Maps authentication token → (session_id, creation_time)
    pending_auth: HashMap<String, (Uuid, Instant)>,

    /// Maps session_id → oneshot Sender to deliver the connection
    session_channels: HashMap<Uuid, oneshot::Sender<Arc<kyproto::Connection>>>,
}

impl IncomingConnectionRouter {
    pub(crate) fn new() -> Self {
        Self {
            pending_auth: HashMap::new(),
            session_channels: HashMap::new(),
        }
    }

    /// Registers a session: adds pending auth token and creates the oneshot channel.
    /// Returns the Receiver that should be stored in KymuxBackend.
    pub(crate) fn register_session(
        &mut self,
        token: String,
        session_id: Uuid,
    ) -> oneshot::Receiver<Arc<kyproto::Connection>> {
        self.cleanup_expired();

        // Add pending auth
        self.pending_auth
            .insert(token, (session_id, Instant::now()));

        // Create channel and store Sender
        let (tx, rx) = oneshot::channel();
        self.session_channels.insert(session_id, tx);

        rx
    }

    /// Removes and returns a pending auth token if it exists and hasn't expired.
    fn take_pending_auth(&mut self, token: &str) -> Option<Uuid> {
        let (session_id, created_at) = self.pending_auth.remove(token)?;

        if is_token_expired(&created_at) {
            warn!("Pending auth token expired for session {}", session_id);
            // Also cleanup the channel
            self.session_channels.remove(&session_id);
            return None;
        }

        Some(session_id)
    }

    /// Removes expired pending auth tokens and their channels.
    pub(crate) fn cleanup_expired(&mut self) {
        let expired: Vec<(String, Uuid)> = self
            .pending_auth
            .iter()
            .filter(|(_, (_, created_at))| is_token_expired(created_at))
            .map(|(token, (session_id, _))| (token.clone(), *session_id))
            .collect();

        for (token, session_id) in expired {
            self.pending_auth.remove(&token);
            self.session_channels.remove(&session_id);
            warn!(
                "Cleaned up expired pending auth token for session {}",
                session_id
            );
        }
    }

    /// Routes a connection to the appropriate session based on the token.
    fn route_connection(&mut self, connection: Arc<kyproto::Connection>, token: &str) -> bool {
        // Validate token and get session_id
        let Some(session_id) = self.take_pending_auth(token) else {
            warn!("Unknown or expired token, cannot route connection");
            return false;
        };

        // Get the channel for this session
        let Some(tx) = self.session_channels.remove(&session_id) else {
            warn!("No channel for session {}, connection dropped", session_id);
            return false;
        };

        // Send connection to the session
        if tx.send(connection).is_err() {
            warn!(
                "Session {} receiver dropped, connection dropped",
                session_id
            );
            return false;
        }

        info!("Connection routed to session {}", session_id);
        true
    }
}

struct KymuxServerTask {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,
}

pub(crate) struct KymuxServer {
    server: Arc<dyn kynet::Server>,
    task: Option<KymuxServerTask>,
}

impl KymuxServer {
    pub(crate) async fn new(
        server: Box<dyn kynet::Server>,
        router: Arc<Mutex<IncomingConnectionRouter>>,
    ) -> Result<Self> {
        let (stop_tx, stop_rx) = oneshot::channel();

        let server: Arc<dyn kynet::Server> = Arc::from(server);
        let server_task = server.clone();

        let join_handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {}
                ret = Self::run_task(server_task, router) => {
                    if let Err(err) = ret {
                        warn!("KymuxServer task ended with error {err:?}");
                    }
                }
            }
        });

        Ok(Self {
            server,
            task: Some(KymuxServerTask {
                stop_tx,
                join_handle,
            }),
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        if let Some(task) = self.task {
            let _ = task.stop_tx.send(());
            task.join_handle.await.map_err(|err| {
                warn!("KymuxServer panicked: {err:?}");
                Error::ThreadPanicked
            })?;
        }

        self.server.close(0, "Server stopping");
        self.server.wait_idle().await;

        Ok(())
    }

    /// Number of concurrent accept tasks. If one is stuck on a misbehaving
    /// client, the others keep serving new connections.
    const ACCEPT_POOL_SIZE: usize = 4;

    /// Accept loop using concurrent tasks so a single misbehaving client
    /// cannot block the entire server from accepting new connections.
    async fn run_task(
        server: Arc<dyn kynet::Server>,
        router: Arc<Mutex<IncomingConnectionRouter>>,
    ) -> Result<()> {
        info!("Waiting for Kymux connections");

        let mut set = task::JoinSet::new();

        loop {
            while set.len() < Self::ACCEPT_POOL_SIZE {
                let server = server.clone();
                let router = router.clone();
                set.spawn(Self::handle_connection(server, router));
            }

            match set.join_next().await {
                Some(Ok(())) => {}
                Some(Err(err)) => error!("Connection handler panicked: {err}"),
                None => break,
            }
        }

        Ok(())
    }

    async fn handle_connection(
        server: Arc<dyn kynet::Server>,
        router: Arc<Mutex<IncomingConnectionRouter>>,
    ) {
        let kynet = match server.accept().await {
            Ok(kynet) => kynet,
            Err(err) => {
                warn!("Kynet accept failed: {err}");
                return;
            }
        };
        let unauth_connection = match kyproto::Connection::accept_with_auth(kynet).await {
            Ok(unauth_connection) => unauth_connection,
            Err(err) => {
                warn!("Kyproto accept failed: {err}");
                return;
            }
        };

        let token = unauth_connection.get_auth().token().to_string();

        // Check token before accepting
        {
            let router = router.lock().await;
            if !router.pending_auth.contains_key(&token) {
                warn!("Rejecting connection with unknown token");
                unauth_connection.reject_authentication();
                return;
            }
        }

        // Accept connection
        let connection = match unauth_connection.accept_authentication().await {
            Ok(conn) => Arc::new(conn),
            Err(err) => {
                warn!("Accept authentication failed: {err}");
                return;
            }
        };

        info!("Kymux connection accepted");

        // Route to session
        let mut router = router.lock().await;
        if !router.route_connection(connection, &token) {
            warn!("Failed to route connection");
        }
    }
}
