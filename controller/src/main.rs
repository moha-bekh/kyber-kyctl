use actix_web::{middleware, post, web, App, HttpResponse, HttpServer};
use log::{info, LevelFilter};
use tokio::sync::Mutex;

mod avservice;
mod controller;
mod error;
mod inputservice;
mod rtp;
mod tls;
mod utils;

use controller::Controller;
use error::{Error, Result};

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

// Load TLS config
//
// /!\/!\ WARNING /!\/!\
// 1. This TLS configuration is only valid for tests.
//    It must NOT be used in production
// 2. Those files are provided by the project that packages the final bundle
fn load_tls_config(cert_path: &str, key_path: &str) -> Result<rustls::ServerConfig> {
    let config = tls::load_config(cert_path, key_path)?;

    let certificate = rustls::Certificate(config.certificate);
    let private_key = rustls::PrivateKey(config.private_key);

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

    let ctrl = web::Data::new(Mutex::new(Controller::new()));
    let tls_config = load_tls_config("kybertest_cert.pem", "kybertest_key.pem").unwrap();

    HttpServer::new(move || {
        App::new()
            .app_data(ctrl.clone())
            .app_data(web::JsonConfig::default().limit(1024))
            .wrap(middleware::Logger::default())
            // RTP
            .service(
                web::scope("/rtp")
                    .service(rtp::requests::hello)
                    .service(rtp::requests::generate_sdp)
                    .service(rtp::requests::start),
            )
            .service(stop_session)
    })
    .bind_rustls("0.0.0.0:8080", tls_config)
    .unwrap()
    .run()
    .await
    .unwrap();
}
