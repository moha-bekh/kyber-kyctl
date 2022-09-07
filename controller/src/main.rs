use log::{info, LevelFilter};

use tokio::sync::Mutex;

use actix_web::{get, middleware, post, web, App, HttpRequest, HttpResponse, HttpServer};

mod controller;
mod error;
mod input_server;
mod requests;
mod streamer_video;
mod utils;

use controller::Controller;
use requests::{GenerateSdpRequest, GenerateSdpResponse, StartRequest, StartResponse};

#[get("/hello")]
async fn hello(req: HttpRequest) -> HttpResponse {
    let peer_addr = if let Some(peer_addr) = req.peer_addr() {
        peer_addr
    } else {
        return HttpResponse::Forbidden().finish();
    };

    HttpResponse::Ok().json(StartResponse {
        ip: peer_addr.ip().to_string(),
    })
}

#[post("/generate_sdp")]
async fn generate_sdp(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<GenerateSdpRequest>,
) -> HttpResponse {
    let ctrl = ctrl.lock().await;

    let ret = ctrl.generate_video_sdp(request.into_inner()).await;
    match ret {
        Ok(params) => {
            info!("SDP generated: {}", params);

            HttpResponse::Ok().json(GenerateSdpResponse { params })
        }
        Err(err) => {
            info!("SDP generation rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

#[post("/start")]
async fn start(
    ctrl: web::Data<Mutex<Controller>>,
    request: web::Json<StartRequest>,
) -> HttpResponse {
    let mut ctrl = ctrl.lock().await;

    let ret = ctrl.start(request.into_inner()).await;
    match ret {
        Ok(_) => {
            info!("Stream started");

            HttpResponse::Ok().finish()
        }
        Err(err) => {
            info!("Stream start rejected: {:?}", err);
            HttpResponse::Forbidden().finish()
        }
    }
}

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
            // enable logger
            .wrap(middleware::Logger::default())
            // register simple handler
            .service(hello)
            .service(generate_sdp)
            .service(start)
            .data(web::JsonConfig::default().limit(1024))
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
