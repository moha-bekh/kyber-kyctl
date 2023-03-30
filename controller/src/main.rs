use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use actix_web::{middleware, post, web, App, HttpResponse, HttpServer};
use log::{info, LevelFilter};
use tokio::sync::Mutex;

mod avservice;
mod controller;
mod error;
mod inputservice;
mod kymux_controller;
mod rtp;
mod tls;
mod utils;

use libkypc::process::ProcessFactory;

use controller::Controller;
use error::{Error, Result};

const LISTENING_PORT: u16 = 8080;

const STREAMER_CERT: &str = "kybertest_cert.pem";
const STREAMER_KEY: &str = "kybertest_key.pem";

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

#[actix_web::main]
async fn main() {
    env_logger::Builder::new()
        .filter_level(LevelFilter::Info)
        .init();

    // Load TLS config
    //
    // /!\/!\ WARNING /!\/!\
    // 1. This TLS configuration is only valid for tests.
    //    It must NOT be used in production
    // 2. Those files are provided by the project that packages the final bundle
    let tls_config = tls::load_config(STREAMER_CERT, STREAMER_KEY).unwrap();
    let server_config = build_serverconfig(&tls_config).unwrap();

    let process_factory = Arc::new(ProcessFactory::new(false));

    let ctrl = web::Data::new(Mutex::new(Controller::new(process_factory, tls_config)));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), LISTENING_PORT);

    HttpServer::new(move || {
        App::new()
            .app_data(ctrl.clone())
            .app_data(web::JsonConfig::default().limit(1024))
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
            .service(stop_session)
    })
    .bind_rustls(bind_addr, server_config)
    .unwrap()
    .run()
    .await
    .unwrap();
}
