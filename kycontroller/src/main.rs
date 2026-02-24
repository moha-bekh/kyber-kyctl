// Project Kyber: main.rs
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

#![cfg_attr(feature = "win32-hide-console", windows_subsystem = "windows")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;

use actix_session::storage::CookieSessionStore;
use actix_session::SessionMiddleware;
use actix_web::{
    cookie, get, middleware, post, web, App, HttpMessage, HttpRequest, HttpResponse, HttpServer,
};
use log::{error, info, warn, LevelFilter};
use serde::{Deserialize, Serialize};
use session::identity::Identity;
use tokio::sync::Mutex;

mod auth;
mod avservice;
mod config;
mod controller;
mod error;
mod host_events;
mod inputservice;
mod kymux_controller;
mod rtp;
mod security;
mod session;
mod statemachine;
mod tls;
mod utils;
mod watchdog;
mod webclient;
mod ws;

use libkypc::process::ProcessFactory;

use controller::{ClientSession, Controller, DataPlaneBackend, StreamingSession};
use error::{Error, Result};
use host_events::{HostEvent, HostEventSender};
use statemachine::{StateMachine, StateMachineEvent, StateMachineProxy};
use utils::TypeConvert;

const WEBCLIENT_ROOT: &str = "webclient";
const STREAMER_CERT: &str = "kybertest_cert.pem";
const STREAMER_KEY: &str = "kybertest_key.pem";

#[derive(Debug, Serialize)]
struct ControlPlaneCapabilities {
    keepintvl: u64,
    keepcnt: u64,
}

#[derive(Debug, Serialize)]
struct Capabilities {
    control_plane: ControlPlaneCapabilities,
}

/// POST /session/login - Authenticates a user and creates a new session.
///
/// This is the main entry point for user authentication. It validates credentials,
/// creates a session cookie, and generates one-time tokens for WebSocket authentication.
///
/// ## Request
///
/// Requires an `Authorization` header with credentials in one of the supported formats:
/// - `Bearer <jwt>` - JWT authentication
/// - `Basic <base64>` - Basic authentication (development only)
///
/// ## Response
///
/// On success (200 OK):
/// ```json
/// {
///   "uid": "9c5b94b1-35ad-49bb-b118-8e8fc24abf80",
///   "websocket": "<128-char-token>"
/// }
/// ```
///
/// The response includes:
/// - `uid`: Session UUID (also stored in encrypted cookie)
/// - `websocket`: One-time token for WebSocket authentication
///
/// A session cookie is automatically set via actix-session middleware.
///
/// ## Error Responses
///
/// - `400 Bad Request`: Invalid or unsupported Authorization header format
/// - `401 Unauthorized`: Authentication failed (invalid credentials, expired JWT, etc.)
///
/// ## Single-Session Enforcement
///
/// If an existing session is active, it's automatically terminated before creating
/// the new session. This enforces a single-session-per-instance policy.
#[post("/login")]
async fn login(ctrl: web::Data<Mutex<Controller>>, req: HttpRequest) -> HttpResponse {
    let Some(Ok((auth_type, auth_creds))) = req
        .headers()
        .get(actix_web::http::header::AUTHORIZATION)
        .map(auth::from_authorization_header)
    else {
        return HttpResponse::BadRequest().json("Invalid authentication type");
    };

    let auth_session = match auth::authenticate(&auth_type, &auth_creds) {
        Ok(session) => session,
        Err(err) => {
            error!("Authentication failed: {err}");
            return HttpResponse::Unauthorized().finish();
        }
    };

    info!("Logging in \"{}\"...", auth_session.username);
    let uuid = Identity::login(auth_session.expiration, &req.extensions()).unwrap();
    let client_session = ClientSession::new(uuid, auth_session);
    let body = serde_json::json!({
        "uid": uuid,
        "websocket": client_session.auth_tokens.websocket,
    });

    let mut ctrl = ctrl.lock().await;

    // Multi-session support: allow multiple simultaneous sessions
    ctrl.sessions.insert(uuid, client_session);

    HttpResponse::Ok().json(body)
}

/// POST /session/renew - Extends an existing session with refreshed credentials.
///
/// This endpoint allows users to renew their session without a full re-login by
/// providing refreshed JWT credentials. It updates the session expiration time
/// in both the cookie and the Controller state.
///
/// ## Request
///
/// Requires both:
/// 1. Valid session cookie (checked via `Identity` parameter)
/// 2. `Authorization` header with refreshed credentials
///
/// The credentials must have an expiration time (JWT only - Basic auth sessions
/// cannot be renewed as they don't expire).
///
/// ## Validation Checks
///
/// The endpoint performs multiple security checks:
/// - Session cookie must be valid and not expired
/// - New credentials must authenticate successfully
/// - New credentials must have an expiration time
/// - Username in new credentials must match the existing session
///
/// This prevents credential swapping attacks where a user tries to renew another
/// user's session with their own credentials.
///
/// ## Response
///
/// - `200 OK`: Session renewed successfully (empty body)
/// - `400 Bad Request`: Invalid Authorization header format
/// - `401 Unauthorized`: Authentication failed or validation check failed
///
/// ## Note on Cookie-Only Access
///
/// Currently, this endpoint requires an existing session cookie, so external clients
/// with only a JWT (no cookie) cannot renew. This is documented as an open design
/// question in Auth.md.
#[post("/renew")]
async fn renew(
    ctrl: web::Data<Mutex<Controller>>,
    req: HttpRequest,
    user: Identity,
) -> HttpResponse {
    let Some(Ok((auth_type, auth_creds))) = req
        .headers()
        .get(actix_web::http::header::AUTHORIZATION)
        .map(auth::from_authorization_header)
    else {
        return HttpResponse::BadRequest().json("Invalid authentication type");
    };

    let auth_session = match auth::authenticate(&auth_type, &auth_creds) {
        Ok(session) => session,
        Err(err) => {
            error!("Authentication failed: {err}");
            return HttpResponse::Unauthorized().finish();
        }
    };

    let Some(exp) = auth_session.expiration else {
        error!("Missing expiration date");
        return HttpResponse::Unauthorized().finish();
    };

    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    let mut ctrl = ctrl.lock().await;
    let Ok(session) = ctrl.get_session(&uid) else {
        warn!("Session destroyed during renewal");
        user.logout();
        return HttpResponse::Unauthorized().finish();
    };

    // Prevent credential swapping: username must match existing session
    if session.username != auth_session.username {
        error!("Credentials and renewed session mismatch");
        return HttpResponse::Unauthorized().finish();
    }

    let Ok(_) = user.renew(exp) else {
        return HttpResponse::Unauthorized().finish();
    };
    session.expiration = Some(exp);

    HttpResponse::Ok().finish()
}

#[get("/capabilities")]
async fn capabilities() -> HttpResponse {
    let ret = Capabilities {
        control_plane: ControlPlaneCapabilities {
            keepintvl: host_events::KEEPINTVL.as_secs(),
            keepcnt: host_events::KEEPCNT,
        },
    };

    HttpResponse::Ok().json(ret)
}

/// GET /ws - WebSocket upgrade endpoint for control plane connection.
///
/// This endpoint implements a two-phase WebSocket authentication flow that avoids
/// exposing credentials in URLs (which would appear in server logs).
///
/// ## Authentication Flow
///
///  Phase 1: HTTP Authentication
///
///   Client → POST /session/login (Authorization header)
///        ← 200 OK {"uid": "<uuid>", "websocket": "<one-time-token>"}
///        ← Set-Cookie: session=<encrypted-cookie>
///
/// Phase 2: WebSocket Connection & Authentication
///
///   Client → GET /ws (WebSocket upgrade request)
///        ← 101 Switching Protocols (WebSocket established)
///
///   [Connection accepted but NOT yet authorized - pending state]
///
///   Client → WS Message: {"token": "<one-time-token>"}
///        ↓
///   Server validates token:
///        - Must match ClientSession.auth_tokens.websocket
///        - Must have active ClientSession
///        - Token is consumed (one-time use)
///
///   If valid:
///        ← WS Message: "Welcome"
///        ← WS Message: DisplayList event
///        [Connection fully authorized - HostEventSender created]
///
///   If invalid:
///        ← WS Close: Protocol error "Invalid credentials"
///
/// ## Why Two Phases?
///
/// **Problem**: Browser security prevents sending cookies during WebSocket upgrades
/// across different origins, and putting tokens in URLs risks exposure in logs.
///
/// **Solution**: "Credentials as first message" pattern:
/// 1. Accept WebSocket at transport level (no auth yet)
/// 2. Wait for first message containing one-time token
/// 3. Validate token against HTTP session
/// 4. Either activate (accept) or terminate (reject) the connection
///
/// ## Implementation Details
///
/// This handler returns immediately with the WebSocket upgrade response (HTTP 101).
/// Authentication happens asynchronously in a spawned task:
///
/// 1. **Accept Transport**: `actix_ws::handle()` upgrades to WebSocket
/// 2. **Spawn Auth Task**: Background task waits for first message
/// 3. **Wait for Token**: `wait_authentication()` with 5-second timeout
/// 4. **Validate Token**: Compare against `ClientSession.auth_tokens.websocket`
/// 5. **Accept/Reject**: Either promote to `HostEventSender` or close connection
#[get("/ws")]
async fn websocket_service(
    ctrl: web::Data<Mutex<Controller>>,
    req: HttpRequest,
    stream: web::Payload,
) -> HttpResponse {
    let (http_response, session, msg_stream) = match actix_ws::handle(&req, stream) {
        Ok((http_response, session, msg_stream)) => (http_response, session, msg_stream),
        Err(err) => {
            warn!("Fail to start websocket: {err:?}");
            return HttpResponse::Forbidden().finish();
        }
    };

    // Spawn async task for authentication to avoid blocking HTTP worker
    tokio::task::spawn_local(async move {
        let auth = host_events::wait_authentication(session, msg_stream).await;
        let (token, pending_sender) = match auth {
            Ok(values) => values,
            Err(e) => {
                error!("Websocket initialization failed: {e:?}");
                return;
            }
        };

        let mut ctrl = ctrl.lock().await;

        let Some(state_machine_proxy) = ctrl.state_machine_proxy.clone() else {
            warn!("StateMachineProxy not configured");
            return;
        };

        let displays = ctrl.displays.convert();

        // Find the session that matches this websocket token
        let session_entry = ctrl.sessions.iter_mut().find(|(_, session)| {
            !session.auth_tokens.websocket.is_empty() && session.auth_tokens.websocket == token
        });

        let Some((session_id, client_session)) = session_entry else {
            error!("Client websocket token is invalid: \"{token}\"");
            pending_sender.reject().await;
            return;
        };

        let session_id = *session_id;

        // Token is valid, consume it (one-time use)
        client_session.auth_tokens.websocket.clear();
        info!(
            "Websocket token validated and consumed for session {}",
            session_id
        );

        let Ok(sender) = pending_sender.accept(session_id, state_machine_proxy).await else {
            error!("Could not accept websocket connection");
            return;
        };

        sender.send(HostEvent::DisplayList { displays }).await;

        client_session.stream_session = Some(StreamingSession::new(sender));
        info!("Session \"{}\" controlplane connected", client_session.uid);
    });

    http_response
}

#[get("/enumerate_displays")]
async fn enumerate_displays(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
    let ctrl = ctrl.lock().await;

    HttpResponse::Ok().json(ctrl.displays.clone())
}

#[post("/stop_streaming")]
async fn stop_streaming(ctrl: web::Data<Mutex<Controller>>, user: Identity) -> HttpResponse {
    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    let mut ctrl = ctrl.lock().await;

    let ret = ctrl.stop_streaming(uid).await;
    match ret {
        Ok(_) => {
            info!("Stream stopped");
            HttpResponse::Ok().finish()
        }
        Err(err) => {
            info!("Stream stop rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[post("/logout")]
async fn logout(ctrl: web::Data<Mutex<Controller>>, user: Identity) -> HttpResponse {
    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    user.logout();

    let mut ctrl = ctrl.lock().await;

    let ret = ctrl.stop(uid).await;
    match ret {
        Ok(_) => {
            info!("Session stopped");
            HttpResponse::Ok().finish()
        }
        Err(err) => {
            info!("Session stop rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SetBitrate {
    // Global bitrate for the whole Quic connection. Unit: bits/sec
    pub(crate) bitrate: u32,
}

#[post("/set_bitrate")]
pub(crate) async fn set_bitrate(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<SetBitrate>,
    user: Identity,
) -> HttpResponse {
    info!("Set bitrate: {}", request.bitrate);

    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    let mut ctrl = ctrl.lock().await;

    let Some(session) = ctrl.sessions.get_mut(&uid) else {
        warn!("Session not found");
        return HttpResponse::Forbidden().finish();
    };

    if session.services.video.is_empty() {
        warn!("No video streaming started");
        return HttpResponse::Forbidden().finish();
    }

    let splitted_bitrate = request.bitrate / session.services.video.len() as u32;

    for avserver in &mut session.services.video {
        let ret = avserver.video_set_bitrate(splitted_bitrate as _).await;
        if let Err(err) = ret {
            warn!("Failed to set bitrate for one video avserver: {err:?}");
        }
    }

    if let Some(stream_session) = session.stream_session.as_ref() {
        stream_session
            .host_event_sender
            .send(HostEvent::BitrateChanged {
                bitrate: request.bitrate,
            })
            .await;
    }

    HttpResponse::Ok().finish()
}

fn build_serverconfig(tls_config: &tls::Config) -> Result<rustls::ServerConfig> {
    Ok(rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            tls_config.cert_chain.clone(),
            tls_config.private_key.clone_key(),
        )?)
}

fn setup_logs(logfile: &str, level: LevelFilter) -> log4rs::Handle {
    use std::path::Path;

    use log4rs::{
        append::{console::ConsoleAppender, file::FileAppender},
        config::{Appender, Config, Root},
        encode::pattern::PatternEncoder,
    };

    // Setup pattern
    let pattern_encoder = PatternEncoder::new("{d(%Y-%m-%dT%H:%M:%S.%3f)} {h({l})} {t} - {m}{n}");

    // Setup Console output
    let stdout = ConsoleAppender::builder()
        .encoder(Box::new(pattern_encoder.clone()))
        .build();

    // Compute log file
    let log_folder = "log";
    let log_file = format!("{logfile}.log");

    let log_path = if cfg!(windows) {
        let mut exe_path = std::env::current_exe().unwrap();
        exe_path.pop();

        Path::new(&exe_path).join(log_folder).join(log_file)
    } else {
        Path::new(log_folder).join(log_file)
    };

    // Setup File output
    let file = FileAppender::builder()
        .encoder(Box::new(pattern_encoder))
        .build(log_path)
        .unwrap();

    // Configure logger
    let config = Config::builder()
        .appender(Appender::builder().build("stdout", Box::new(stdout)))
        .appender(Appender::builder().build("file", Box::new(file)))
        .build(
            Root::builder()
                .appender("file")
                .appender("stdout")
                .build(level),
        )
        .unwrap();

    log4rs::init_config(config).unwrap()
}

async fn real_main() -> Result<()> {
    // Initialize rustls crypto
    let ret = rustls::crypto::ring::default_provider().install_default();
    if ret.is_err() {
        warn!("Rustls default provider init failed");
        return Err(Error::RustlsDefaultProviderInit);
    }

    // Load configuration
    let config = config::load()?;

    // Load TLS config
    let tls_cert = config.tls_cert().unwrap_or(STREAMER_CERT);
    let tls_key = config.tls_key().unwrap_or(STREAMER_KEY);
    let tls_config = tls::load_config(tls_cert, tls_key)?;
    let server_config = build_serverconfig(&tls_config)?;

    let sec = config.security();
    let trusted_origins = sec.trusted_origins().clone();
    let cookie_same_site = sec.same_site();

    auth::init(config.auth());

    let process_factory = Arc::new(ProcessFactory::new(false));

    let ctrl = web::Data::new(Mutex::new(Controller::new(
        process_factory.clone(),
        config,
        tls_config,
    )));

    let statemachine = StateMachine::new(ctrl.clone())?;

    let (watchdog, listening_port) = {
        let mut ctrl = ctrl.lock().await;

        // Configure watchdog
        info!("Starting watchdog");
        let (watchdog, displays) =
            watchdog::Watchdog::new(&process_factory, statemachine.create_proxy()).await?;

        info!("Got initial displays: {displays:?}");
        ctrl.displays = displays;

        // Configure StateMachine listener
        ctrl.state_machine_proxy = Some(statemachine.create_proxy());

        (watchdog, ctrl.config.port())
    };

    let session_exp_task = session::expiration::ExpirationTask::new(ctrl.clone());

    info!("HTTP listening on port {listening_port}");
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listening_port);

    // Serve webclient if found
    let webclient_root = webclient::root();
    if let Some(webclient_root) = &webclient_root {
        info!("Serving webclient from {webclient_root}");
    } else {
        info!("Webclient root not found");
    }

    let cookie_key = cookie::Key::generate();

    HttpServer::new(move || {
        // Actix Middleware Stack: Middleware executes in REVERSE order of .wrap() calls
        //
        // Data Flow:
        // Cookie → Session → IdentityInner → Identity (FromRequest)
        let app = App::new()
            .app_data(ctrl.clone())
            .app_data(web::JsonConfig::default().limit(1024))
            // Custom middleware: wraps Session in IdentityInner, enforces expiration
            .wrap(session::identity::middleware::IdentityMiddleware {})
            // Decrypts session cookie, provides Session object to downstream middleware
            .wrap(
                SessionMiddleware::builder(CookieSessionStore::default(), cookie_key.clone())
                    .cookie_same_site(cookie_same_site)
                    .cookie_name("__HOST-SESSION".to_owned())
                    .build(),
            )
            .wrap(security::cop::CrossOriginProtection::new(
                trusted_origins.clone(),
            ))
            .wrap(security::build_cors(&trusted_origins))
            .wrap(middleware::Logger::default())
            // Kymux
            .service(
                web::scope("/kymux")
                    .service(kymux_controller::requests::start_mux)
                    .service(kymux_controller::requests::start_session),
            )
            // RTP
            .service(
                web::scope("/rtp")
                    .service(rtp::requests::hello)
                    .service(rtp::requests::generate_sdp)
                    .service(rtp::requests::start),
            )
            // Websocket
            .service(
                web::scope("/websocket")
                    .service(ws::requests::start_handler)
                    .service(ws::requests::inputs)
                    .service(ws::requests::video)
                    .service(ws::requests::audio),
            )
            .service(
                web::scope("/session")
                    .service(login)
                    .service(renew)
                    .service(logout),
            )
            .service(capabilities)
            .service(websocket_service)
            .service(enumerate_displays)
            .service(stop_streaming)
            .service(set_bitrate);

        if let Some(webclient_root) = &webclient_root {
            app.service(
                web::scope("/webclient")
                    .wrap(webclient::WebClient)
                    .service(actix_files::Files::new("", webclient_root).index_file("index.html")),
            )
        } else {
            app
        }
    })
    .bind_rustls_0_23(bind_addr, server_config)?
    .run()
    .await?;

    session_exp_task.stop().await;
    watchdog.stop().await?;
    statemachine.stop().await?;

    Ok(())
}

#[actix_web::main]
async fn main() -> ExitCode {
    let _log_handle = setup_logs("kycontroller", LevelFilter::Info);
    info!("Kyber controller. Revision {}", env!("VERGEN_GIT_DESCRIBE"));

    match real_main().await {
        Err(e) => {
            error!("Main process exited with error: {e}");
            e.exit_code()
        }
        Ok(_) => ExitCode::SUCCESS,
    }
}
