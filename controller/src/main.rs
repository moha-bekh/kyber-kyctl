#![cfg_attr(feature = "win32-hide-console", windows_subsystem = "windows")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use actix_web::{get, middleware, post, web, App, HttpRequest, HttpResponse, HttpServer};
use log::{info, warn, LevelFilter};
use tokio::sync::Mutex;

mod avservice;
mod controller;
mod error;
mod host_events;
mod inputservice;
mod kymux_controller;
mod rtp;
mod statemachine;
mod tls;
mod utils;
mod watchdog;
mod webclient;

use libkypc::process::ProcessFactory;

use controller::Controller;
use error::{Error, Result};
use host_events::{HostEvent, HostEventSender};
use statemachine::{StateMachine, StateMachineEvent, StateMachineProxy};
use utils::TypeConvert;

const LISTENING_PORT: u16 = 8080;
const WEBCLIENT_ROOT: &str = "webclient";
const STREAMER_CERT: &str = "kybertest_cert.pem";
const STREAMER_KEY: &str = "kybertest_key.pem";

#[get("/ws")]
async fn websocket_service(
    ctrl: web::Data<Mutex<Controller>>,
    req: HttpRequest,
    stream: web::Payload,
) -> HttpResponse {
    let (host_event_sender, http_response) = match HostEventSender::new(req, stream) {
        Ok((host_event_sender, http_response)) => (host_event_sender, http_response),
        Err(err) => {
            warn!("Fail to start websocket: {err:?}");
            return HttpResponse::Forbidden().finish();
        }
    };

    let mut ctrl = ctrl.lock().await;

    host_event_sender
        .send(HostEvent::DisplayList {
            displays: ctrl.displays.convert(),
        })
        .await;

    ctrl.host_event_sender = Some(host_event_sender);

    http_response
}

#[get("/enumerate_displays")]
async fn enumerate_displays(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
    let ctrl = ctrl.lock().await;

    HttpResponse::Ok().json(ctrl.displays.clone())
}

#[post("/stop_session")]
async fn stop_session(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    let ret = ctrl.stop().await;
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

fn build_serverconfig(tls_config: &tls::Config) -> Result<rustls::ServerConfig> {
    let certificate = rustls::Certificate(tls_config.certificate.clone());
    let private_key = rustls::PrivateKey(tls_config.private_key.clone());

    // Build configuration
    Ok(rustls::ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)?)
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

#[actix_web::main]
async fn main() {
    let _log_handle = setup_logs("controller", LevelFilter::Info);

    info!("Kyber controller. Revision {}", env!("VERGEN_GIT_DESCRIBE"));

    // Load TLS config
    //
    // /!\/!\ WARNING /!\/!\
    // 1. This TLS configuration is only valid for tests.
    //    It must NOT be used in production
    // 2. Those files are provided by the project that packages the final bundle
    let tls_config = tls::load_config(STREAMER_CERT, STREAMER_KEY).unwrap();
    let server_config = build_serverconfig(&tls_config).unwrap();

    let process_factory = Arc::new(ProcessFactory::new(false));

    let ctrl = web::Data::new(Mutex::new(Controller::new(
        process_factory.clone(),
        tls_config,
    )));

    let statemachine = StateMachine::new(ctrl.clone()).unwrap();

    let watchdog = {
        let mut ctrl = ctrl.lock().await;

        let (watchdog, displays) =
            watchdog::Watchdog::new(&process_factory, statemachine.create_proxy())
                .await
                .unwrap();

        ctrl.displays = displays;

        watchdog
    };

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), LISTENING_PORT);

    // Serve webclient if found
    let webclient_root = webclient::root();
    if let Some(webclient_root) = &webclient_root {
        info!("Serving webclient from {webclient_root}");
    } else {
        info!("Webclient root not found");
    }

    HttpServer::new(move || {
        let app = App::new()
            .app_data(ctrl.clone())
            .app_data(web::JsonConfig::default().limit(1024))
            .wrap(middleware::Logger::default())
            .wrap(
                actix_cors::Cors::default()
                    .allowed_methods(vec!["GET", "POST"])
                    .allow_any_origin()
                    .allowed_header(actix_web::http::header::CONTENT_TYPE),
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
            .service(websocket_service)
            .service(enumerate_displays)
            .service(stop_session);

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
    .bind_rustls_021(bind_addr, server_config)
    .unwrap()
    .run()
    .await
    .unwrap();

    watchdog.stop().await.unwrap();
    statemachine.stop().await.unwrap();
}
