use log::{info, LevelFilter};

use tokio::sync::Mutex;

use actix_web::{middleware, post, web, App, HttpResponse, HttpServer};

mod controller;
mod error;
mod input_server;
mod rtp;
mod streamer_audio;
mod streamer_video;
mod utils;

use controller::Controller;

#[post("/stop")]
async fn stop(ctrl: web::Data<Mutex<Controller>>) -> HttpResponse {
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

pub async fn run() -> std::io::Result<()> {
    std::env::set_var("RUST_LOG", "actix_web=info");

    let ctrl = web::Data::new(Mutex::new(Controller::new()));

    // move is necessary to give closure below ownership of counter1
    HttpServer::new(move || {
        App::new()
            .app_data(ctrl.clone()) // add shared state
            .app_data(web::JsonConfig::default().limit(1024))
            // enable logger
            .wrap(middleware::Logger::default())
            // register simple handler
            .service(rtp::requests::hello)
            .service(rtp::requests::generate_sdp)
            .service(rtp::requests::start)
            .service(stop)
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::new()
        .filter_level(LevelFilter::Info)
        .init();

    run().await
}
