use log::{info, LevelFilter};

use tokio::sync::Mutex;

use actix_web::{middleware, post, web, App, HttpResponse, HttpServer};

mod avservice;
mod controller;
mod error;
mod inputservice;
mod rtp;
mod utils;

use controller::Controller;

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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::Builder::new()
        .filter_level(LevelFilter::Info)
        .init();

    let ctrl = web::Data::new(Mutex::new(Controller::new()));

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
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
