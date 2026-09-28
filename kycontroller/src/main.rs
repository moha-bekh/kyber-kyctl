// Project Kyber: main.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use actix_session::SessionMiddleware;
use actix_session::storage::CookieSessionStore;
use actix_web::{
    App, HttpMessage, HttpRequest, HttpResponse, HttpServer, cookie, get, middleware, post, web,
};
use clap::Parser;
use log::{error, info, warn};
use log4ky::notice;
use serde::Deserialize;
use session::identity::Identity;
use toml::Table;

mod auth;
mod avservice;
mod avservices_manager;
mod cli;
mod config;
mod controller;
mod error;
mod host_events;
mod inputservice;
pub(crate) mod kydup;
mod kymux_controller;
mod rtp;
mod security;
mod session;
mod statemachine;
mod tls;
mod tray;
mod utils;
mod watchdog;
mod webclient;

use libkypc::process::ProcessFactory;

use controller::{ClientSession, Controller, DataPlaneBackend};
use error::{Error, Result};
use host_events::HostEvent;
use statemachine::{StateMachineEvent, StateMachineProxy};
use utils::TypeConvert;

const WEBCLIENT_ROOT: &str = "webclient";
const STREAMER_CERT: &str = "kybertest_cert.pem";
const STREAMER_KEY: &str = "kybertest_key.pem";

use kyctl_common::capabilities::{Capabilities, ControlPlaneCapabilities};

#[derive(Debug, Deserialize)]
struct LoginQuery {
    auth: Option<String>,
}

/// POST /session/login - Authenticates a user and creates a new session.
///
/// This is the main entry point for user authentication. It validates credentials,
/// creates a session cookie, and generates one-time tokens for WebSocket authentication.
///
/// ## Request
///
/// Requires an `Authorization` header with credentials in one of the supported formats:
/// - `Bearer <access_token>` - JWT authentication (default)
/// - `Bearer <access_token>` with `?auth=oidc` - OIDC authentication
/// - `Basic <base64>` - Basic authentication (development only)
///
/// ## OIDC Discovery
///
/// When `?auth=oidc` is present but no `Authorization` header is sent, the server returns:
/// ```text
/// 401 Unauthorized
/// WWW-Authenticate: Bearer realm="oidc", issuer="...", client_id="..."
/// ```
/// The client then performs the OIDC flow and retries with the obtained token.
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
/// ## Mono-Session Enforcement
///
/// When `multi_client = true` (default), multiple sessions are accepted.
/// When `multi_client = false`, a second concurrent login attempt is rejected
/// with `409 Conflict` until the existing session logs out.
#[post("/login")]
async fn login(
    ctrl: web::Data<Controller>,
    req: HttpRequest,
    query: web::Query<LoginQuery>,
) -> HttpResponse {
    let auth_hint = query.auth.as_deref();

    // OIDC discovery: ?auth=oidc without Authorization header.
    #[cfg(feature = "auth-oidc")]
    if auth_hint == Some("oidc")
        && !req
            .headers()
            .contains_key(actix_web::http::header::AUTHORIZATION)
    {
        let Some(oidc) = auth::registry().oidc_backend() else {
            return HttpResponse::Unauthorized().json("Disabled authentication scheme");
        };
        return HttpResponse::Unauthorized()
            .insert_header((
                actix_web::http::header::WWW_AUTHENTICATE,
                oidc.www_authenticate_value(),
            ))
            .finish();
    }

    let Some(parser_hdr) = req
        .headers()
        .get(actix_web::http::header::AUTHORIZATION)
        .and_then(|h| auth::AuthorizationHeader::from_header(h, auth_hint))
    else {
        return HttpResponse::BadRequest().json("Invalid authentication type");
    };

    let Some(auth_backend) = auth::registry().backend(parser_hdr.kind) else {
        return HttpResponse::Unauthorized().json("Disabled authentication scheme");
    };

    let auth_session = match auth_backend.authenticate(parser_hdr.creds) {
        Ok(session) => session,
        Err(err @ auth::Error::OidcUnavailable { .. }) => {
            warn!("Authentication unavailable: {err}");
            return HttpResponse::ServiceUnavailable().finish();
        }
        Err(err) => {
            warn!("Authentication failed: {err}");
            return HttpResponse::Unauthorized().finish();
        }
    };

    if ctrl.config.session_mode() == config::SessionMode::Mono
        && !ctrl.sessions.lock().unwrap().is_empty()
    {
        warn!(
            "Refusing login for \"{}\": mono-client mode and a session is already active",
            auth_session.username
        );
        return HttpResponse::Conflict().json("Another client is already connected");
    }

    info!("Logging in \"{}\"...", auth_session.username);
    let uuid = Identity::login(auth_session.expiration, &req.extensions()).unwrap();
    let client_session = ClientSession::new(uuid, auth_session);
    let body = kyctl_common::session::LoginResponse {
        uid: uuid.to_string(),
        websocket: client_session.auth_tokens.websocket.clone(),
    };

    ctrl.insert_session(client_session);

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
#[post("/renew")]
async fn renew(
    ctrl: web::Data<Controller>,
    req: HttpRequest,
    query: web::Query<LoginQuery>,
    user: Identity,
) -> HttpResponse {
    let auth_hint = query.auth.as_deref();
    let Some(parser_hdr) = req
        .headers()
        .get(actix_web::http::header::AUTHORIZATION)
        .and_then(|h| auth::AuthorizationHeader::from_header(h, auth_hint))
    else {
        return HttpResponse::BadRequest().json("Invalid authentication type");
    };

    let Some(auth_backend) = auth::registry().backend(parser_hdr.kind) else {
        return HttpResponse::Unauthorized().json("Disabled authentication scheme");
    };

    let auth_session = match auth_backend.authenticate(parser_hdr.creds) {
        Ok(session) => session,
        Err(err @ auth::Error::OidcUnavailable { .. }) => {
            warn!("Authentication unavailable: {err}");
            return HttpResponse::ServiceUnavailable().finish();
        }
        Err(err) => {
            warn!("Authentication failed: {err}");
            return HttpResponse::Unauthorized().finish();
        }
    };

    let Some(exp) = auth_session.expiration else {
        warn!("Missing expiration date");
        return HttpResponse::Unauthorized().finish();
    };

    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    let mut sessions = ctrl.sessions.lock().unwrap();
    let Some(session) = sessions.get_mut(&uid) else {
        warn!("Session destroyed during renewal");
        user.logout();
        return HttpResponse::Unauthorized().finish();
    };

    // Prevent credential swapping: username must match existing session
    if session.username != auth_session.username {
        warn!("Credentials and renewed session mismatch");
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
/// Validate a WebSocket token under the Controller lock. Returns the session ID,
/// statemachine proxy, and display list if valid. No `.await` while locked.
fn validate_websocket_token(
    ctrl: &web::Data<Controller>,
    token: &str,
) -> Option<(uuid::Uuid, Vec<host_events::Display>)> {
    let displays = ctrl.displays.lock().unwrap().convert();

    let mut sessions = ctrl.sessions.lock().unwrap();
    let (session_id, client_session) = sessions.iter_mut().find(|(_, session)| {
        !session.auth_tokens.websocket.is_empty() && session.auth_tokens.websocket == token
    })?;

    let session_id = *session_id;
    client_session.auth_tokens.websocket.clear();
    info!(
        "Websocket token validated and consumed for session {}",
        session_id
    );

    Some((session_id, displays))
}

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
    ctrl: web::Data<Controller>,
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

        // Validate token under lock
        let Some((session_id, displays)) = validate_websocket_token(&ctrl, &token) else {
            error!("Websocket validation failed: invalid or expired token");
            pending_sender.reject().await;
            return;
        };

        let Ok(sender) = pending_sender
            .accept(session_id, ctrl.state_machine_proxy.clone())
            .await
        else {
            error!("Could not accept websocket connection");
            return;
        };

        sender.send(HostEvent::DisplayList { displays });

        let ret = ctrl.with_session(&session_id, |session| {
            // A control plane already set means a reconnect.
            let step = match session
                .session
                .replace(controller::StreamingSession::new(sender))
            {
                Some(_) => controller::SessionStep::Reconnected,
                None => {
                    #[cfg(feature = "kyhub")]
                    {
                        session.started_at = Some(time::OffsetDateTime::now_utc());
                    }
                    controller::SessionStep::Started
                }
            };
            ctrl.report_session(step, session);
            Ok(())
        });

        if ret.is_ok() {
            info!("Session \"{session_id}\" controlplane connected");
        }
        ctrl.update_tray_status();
    });

    http_response
}

#[get("/enumerate_displays")]
async fn enumerate_displays(ctrl: web::Data<Controller>) -> HttpResponse {
    HttpResponse::Ok().json(ctrl.displays.lock().unwrap().clone())
}

#[post("/stop_streaming")]
async fn stop_streaming(ctrl: web::Data<Controller>, user: Identity) -> HttpResponse {
    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    match ctrl.stop_streaming(&uid).await {
        Ok(()) => {
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
async fn logout(ctrl: web::Data<Controller>, user: Identity) -> HttpResponse {
    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    user.logout();

    match ctrl
        .remove_session(uid, controller::SessionEndReason::UserLogout)
        .await
    {
        Ok(()) => {
            info!("Session stopped");
            HttpResponse::Ok().finish()
        }
        Err(err) => {
            info!("Session stop rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

use kyctl_common::set_bitrate::SetBitrate;

#[post("/set_bitrate")]
pub(crate) async fn set_bitrate(
    ctrl: web::Data<Controller>,
    request: web::Json<SetBitrate>,
    user: Identity,
) -> HttpResponse {
    info!("Set bitrate: {}", request.bitrate);

    let Ok(uid) = user.uid() else {
        return HttpResponse::Unauthorized().finish();
    };

    // Split the global bitrate evenly across `count` streams and cast to the
    // signed type the encoder expects. `count` is assumed non-zero; the call
    // sites guard against an empty stream set first.
    let split_bitrate = |count: usize| -> i32 { (request.bitrate / count as u32) as i32 };

    let services = match ctrl.with_session(&uid, |s| Ok(s.services.clone())) {
        Ok(s) => s,
        Err(err) => {
            warn!("Session not found: {err}");
            return HttpResponse::Forbidden().finish();
        }
    };

    let mut services = services.lock().await;

    // Set bitrate: av_services_manager first (Kymux/WS), fallback to per-session (RTP legacy).
    // The two paths stay separate because SharedHandler::video_set_bitrate is sync
    // (actor-driven) while Handler::video_set_bitrate is async.
    //
    // Scope to this session's video subscriptions so client A can't reconfigure
    // client B's avservice. For Shared entries the underlying avservice is still
    // global; log a warning since the change will affect every other client
    // on the same display (last-writer-wins, temporary).
    if !services.video_subscriptions.is_empty() {
        let per_stream = split_bitrate(services.video_subscriptions.len());
        let av_services_manager = ctrl.av_services_manager.lock().await;

        for sub in &services.video_subscriptions {
            let key = sub.key();

            if matches!(
                key,
                crate::avservices_manager::VideoStreamKey::Shared { .. }
            ) {
                warn!(
                    "set_bitrate on Shared stream {key:?} overrides other clients on this display"
                );
            }

            let Some(stream) = av_services_manager.video_streams.get(key) else {
                continue;
            };
            if let Err(err) = stream.avservice.video_set_bitrate(per_stream) {
                warn!("Failed to set bitrate: {err:?}");
            }
        }
    } else if !services.video.is_empty() {
        let per_stream = split_bitrate(services.video.len());
        for avserver in &mut services.video {
            if let Err(err) = avserver.video_set_bitrate(per_stream).await {
                warn!("Failed to set bitrate: {err:?}");
            }
        }
    }

    let _ = ctrl.with_session(&uid, |s| {
        if let Some(s) = &s.session {
            s.host_event_sender
                .send(host_events::HostEvent::BitrateChanged {
                    bitrate: request.bitrate,
                });
        }
        Ok(())
    });

    HttpResponse::Ok().finish()
}

fn create_listening_socket(config: &config::Config) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let (addr, domain) = match config.listen_mode() {
        config::ListenMode::Ipv4 => (
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), config.port()),
            Domain::IPV4,
        ),
        config::ListenMode::DualStack => (
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), config.port()),
            Domain::IPV6,
        ),
    };

    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    match config.listen_mode() {
        config::ListenMode::Ipv4 => {}
        config::ListenMode::DualStack => {
            // Enable dual-stack: accept both IPv4 and IPv6 on this socket.
            // On Linux this is the default; on Windows it must be set explicitly.
            if let Err(e) = socket.set_only_v6(false) {
                warn!("Unable to make socket dual-stack: {e}");
            }
        }
    }

    // Align with actix-web behavior about SO_REUSEADDR
    // * https://github.com/actix/actix-web/blob/b9d3adfa4d4b70d2a110897adb2207f97e074a77/actix-web/src/server.rs#L1258
    // * https://github.com/actix/actix-web/issues/2958
    #[cfg(not(windows))]
    socket.set_reuse_address(true)?;

    socket.bind(&addr.into())?;

    // Since we are creating a TcpListener ourself, try to match
    // actix_web::HttpServer recommandations. actix_web::HttpServer::backlog()
    // documentation states:
    // > Generally set in the 64–2048 range. Default value is 1024.
    //
    // Arbitrary choice: lowest value since we aren't targeting massing request
    // rate.
    socket.listen(64)?;

    Ok(std::net::TcpListener::from(socket))
}

fn build_serverconfig(tls_config: &tls::Config) -> Result<rustls::ServerConfig> {
    Ok(rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            tls_config.cert_chain.clone(),
            tls_config.private_key.clone_key(),
        )?)
}

async fn real_main(config: config::Config) -> Result<()> {
    // Initialize rustls crypto
    let ret = rustls::crypto::ring::default_provider().install_default();
    if ret.is_err() {
        warn!("Rustls default provider init failed");
        return Err(Error::RustlsDefaultProviderInit);
    }

    // Load TLS config
    let tls_cert = config.tls_cert().unwrap_or(STREAMER_CERT);
    let tls_key = config.tls_key().unwrap_or(STREAMER_KEY);
    let tls_config = tls::load_config(tls_cert, tls_key)?;
    let server_config = build_serverconfig(&tls_config)?;

    let sec = config.security();
    let trusted_origins = sec.trusted_origins().clone();
    let cookie_same_site = sec.same_site();
    sec.warn_if_insecure();

    auth::init(config.auth())
        .await
        .map_err(|reason| Error::InvalidConfig { reason })?;

    let process_factory = Arc::new(ProcessFactory::new(false));

    let listening_port = config.port();
    let listener = create_listening_socket(&config)?;
    let webclient_override = config.webclient().map(PathBuf::from);

    let kycom = kymux::kycom::KyCom::start_on_any_port(Controller::KYCOM_PORTS).await?;
    let (ctrl, _state_machine) =
        Controller::new(process_factory.clone(), config, tls_config, kycom);

    // Spawn the AVServicesManager background tasks (IDR dispatcher, AV reaper).
    // Must run before any session opens. The returned handle carries the
    // cancel signal, dropped at the end of `real_main`, which signals both
    // tasks to exit.
    let av_tasks_handle;
    let _watchdog = {
        let weak = Arc::downgrade(&ctrl.clone().into_inner());

        av_tasks_handle = ctrl.av_services_manager.lock().await.init(weak);

        // Configure watchdog
        if ctrl.config.watchdog() {
            info!("Starting watchdog");
            let (watchdog, displays) =
                watchdog::Watchdog::new(&process_factory, ctrl.state_machine_proxy.clone()).await?;

            info!("Got initial displays: {displays:?}");
            *ctrl.displays.lock().unwrap() = displays;
            Some(watchdog)
        } else {
            warn!("Watchdog disabled in config! Audio and video disabled.");
            None
        }
    };

    let _session_exp_task = session::expiration::expiration_task(ctrl.clone());

    info!("HTTP listening on port {listening_port}");

    // Serve webclient if found
    let webclient_root = webclient::root(webclient_override.as_deref());
    if let Some(webclient_root) = &webclient_root {
        info!("Serving webclient from {webclient_root}");
    } else {
        info!("Webclient root not found");
    }

    let cookie_key = cookie::Key::generate();

    // Clone ctrl for use after the HttpServer closure (which moves ctrl)
    let ctrl_for_tray = ctrl.clone();

    let server =
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
                        .service(kymux_controller::requests::ws_inputs)
                        .service(kymux_controller::requests::ws_video)
                        .service(kymux_controller::requests::ws_audio),
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
                app.service(web::redirect("/webclient", "/webclient/"))
                    .service(web::scope("/webclient").wrap(webclient::WebClient).service(
                        actix_files::Files::new("", webclient_root).index_file("index.html"),
                    ))
            } else {
                app
            }
        })
        .listen_rustls_0_23(listener, server_config)?
        .run();

    let server_handle = server.handle();
    let tray_enabled = tray::is_supported() && ctrl_for_tray.config.tray_enabled();

    tokio::select! {
        result = server => {
            if let Err(e) = result {
                warn!("Server error: {e}");
            }
        }
        _ = tray::run(ctrl_for_tray.clone(), server_handle.clone()), if tray_enabled => {}
        _ = tray::wait_service_shutdown() => {
            info!("Service shutdown signal received");
            ctrl_for_tray.shutdown_tray();
            server_handle.stop(false).await;
        }
    }

    ctrl_for_tray.shutdown_tray();

    av_tasks_handle.stop();

    Ok(())
}

#[actix_web::main]
async fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let path = config::config_path(cli.config.as_deref());

    // Export the config file the controller resolved (an explicit --config,
    // KYBER_CONFIG, or the executable-relative default) so every spawned
    // service reads the same file instead of one next to its own executable.
    // Done before real_main starts the server's worker threads, since the
    // environment is not safe to mutate once other threads run. If no readable
    // file resolves, leave it unset: a missing default stays a soft fallback,
    // while a missing explicit pointer is reported below.
    if let Ok(canonical) = std::fs::canonicalize(&path) {
        unsafe {
            std::env::set_var("KYBER_CONFIG", canonical);
        }
    }

    let parsed = config::parse(cli.config.as_deref());
    let empty_table = Table::new();
    let table_ref = match &parsed {
        Ok(Some(table)) => table,
        _ => &empty_table,
    };
    // Initialize the logger before reporting parse status: a hidden-console
    // Windows build has no stderr, so a parse failure must reach the log
    // file, not disappear. When the file can't be parsed its log section is
    // unreachable anyway, so fall back to the env-derived base config.
    let log_handle = match log4ky::LoggerBuilder::new("kycontroller")
        .toml(table_ref)
        .console(!cfg!(feature = "win32-hide-console"))
        .cli_level(cli.log_level)
        .init()
    {
        Ok(handle) => handle,
        // Config-key warnings already went to stderr and the log file; a
        // hard install failure (no log file exists yet) has not.
        Err(e) => {
            notice!("Failed to initialize logger: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Export the resolved root level so every spawned service (kynputserver)
    // logs at the same level, regardless of whether it came from
    // --log-level, KYBER_LOG_LEVEL, or the config file. Same rationale and
    // thread-safety constraint as the KYBER_CONFIG export above.
    unsafe {
        std::env::set_var("KYBER_LOG_LEVEL", log_handle.level().to_string());
    }
    info!("Kyber controller. Revision {}", env!("VERGEN_GIT_DESCRIBE"));

    let table = match parsed {
        Ok(table) => {
            match &table {
                Some(_) => info!("Using configuration file {path:?}"),
                None => info!("No configuration file found, using defaults"),
            }
            table
        }
        Err(e) => {
            error!("Failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    let cfg = match table
        .as_ref()
        .map(config::from_table)
        .unwrap_or(Ok(config::Config::default()))
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    match real_main(cfg).await {
        Err(e) => {
            error!("Main process exited with error: {e}");
            e.exit_code()
        }
        Ok(_) => ExitCode::SUCCESS,
    }
}
