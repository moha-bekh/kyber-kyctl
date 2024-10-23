#![cfg_attr(feature = "win32-hide-console", windows_subsystem = "windows")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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

mod avservice;
mod config;
mod controller;
mod error;
mod host_events;
mod inputservice;
mod kymux_controller;
mod rtp;
mod session;
mod statemachine;
mod tls;
mod utils;
mod watchdog;
mod webclient;
mod ws;

use libkypc::process::ProcessFactory;

use controller::{ClientSession, Controller, DataPlaneBackend};
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

#[get("/ws")]
async fn websocket_service(
    ctrl: web::Data<Mutex<Controller>>,
    req: HttpRequest,
    stream: web::Payload,
) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    let Some(state_machine_proxy) = ctrl.state_machine_proxy.clone() else {
        warn!("StateMachineProxy not configured");
        return HttpResponse::Forbidden().finish();
    };

    let (host_event_sender, http_response) =
        match HostEventSender::new(req, stream, state_machine_proxy) {
            Ok((host_event_sender, http_response)) => (host_event_sender, http_response),
            Err(err) => {
                warn!("Fail to start websocket: {err:?}");
                return HttpResponse::Forbidden().finish();
            }
        };

    host_event_sender
        .send(HostEvent::DisplayList {
            displays: ctrl.displays.convert(),
        })
        .await;

    ctrl.client_session = Some(ClientSession::new(host_event_sender));

    http_response
}

#[get("/enumerate_displays")]
async fn enumerate_displays(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
    let ctrl = ctrl.lock().await;

    HttpResponse::Ok().json(ctrl.displays.clone())
}

#[post("/stop_streaming")]
async fn stop_streaming(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    let ret = ctrl.stop_streaming().await;
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

#[post("/stop_session")]
async fn stop_session(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    let ret = ctrl.stop().await;
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
) -> HttpResponse {
    info!("Set bitrate: {}", request.bitrate);

    let mut ctrl = ctrl.lock().await;

    if ctrl.services.video.is_empty() {
        warn!("No video streaming started");
        return HttpResponse::Forbidden().finish();
    }

    let splitted_bitrate = request.bitrate / ctrl.services.video.len() as u32;

    for avserver in &mut ctrl.services.video {
        let ret = avserver.video_set_bitrate(splitted_bitrate as _).await;
        if let Err(err) = ret {
            warn!("Failed to set bitrate for one video avserver: {err:?}");
        }
    }

    if let Some(client_session) = &ctrl.client_session {
        client_session
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
    //
    // /!\/!\ WARNING /!\/!\
    // 1. This TLS configuration is only valid for tests.
    //    It must NOT be used in production
    // 2. Those files are provided by the project that packages the final bundle
    let tls_config = tls::load_config(STREAMER_CERT, STREAMER_KEY)?;
    let server_config = build_serverconfig(&tls_config)?;

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

    info!("HTTP listening on port {listening_port}");
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listening_port);

    // Serve webclient if found
    let webclient_root = webclient::root();
    if let Some(webclient_root) = &webclient_root {
        info!("Serving webclient from {webclient_root}");
    } else {
        info!("Webclient root not found");
    }

    HttpServer::new(move || {
        let cookie_key = cookie::Key::generate();
        let app = App::new()
            .app_data(ctrl.clone())
            .app_data(web::JsonConfig::default().limit(1024))
            .wrap(middleware::Logger::default())
            .wrap(
                actix_cors::Cors::default()
                    .allowed_methods(vec!["GET", "POST"])
                    .allow_any_origin()
                    .supports_credentials()
                    .allowed_header(actix_web::http::header::CONTENT_TYPE),
            )
            .wrap(session::identity::middleware::IdentityMiddleware {})
            .wrap(
                SessionMiddleware::builder(CookieSessionStore::default(), cookie_key)
                    .cookie_same_site(cookie::SameSite::Strict)
                    .build(),
            )
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
            .service(capabilities)
            .service(websocket_service)
            .service(enumerate_displays)
            .service(stop_streaming)
            .service(stop_session)
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

    watchdog.stop().await?;
    statemachine.stop().await?;

    Ok(())
}

#[actix_web::main]
async fn main() -> Result<()> {
    let _log_handle = setup_logs("controller", LevelFilter::Info);
    info!("Kyber controller. Revision {}", env!("VERGEN_GIT_DESCRIBE"));

    let ret = real_main().await;
    if let Err(err) = &ret {
        warn!("main() ended with error {err:?}");
    }

    ret
}
