pub(crate) mod requests;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use actix_web::web;
use log::{error, info, warn};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{self, JoinHandle};

use avservice_types::VideoCodec;
use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::Controller;

use crate::LISTENING_PORT;

pub(crate) struct KymuxListener {
    stop_tx: oneshot::Sender<()>,
    join_handle: JoinHandle<()>,
}

impl KymuxListener {
    pub(crate) async fn new(
        listener: kymux::ConnectionListener,
        ctrl: web::Data<Mutex<Controller>>,
    ) -> Result<Self> {
        let (stop_tx, stop_rx) = oneshot::channel();

        let join_handle = task::spawn(async move {
            tokio::select! {
                _ = stop_rx => {
                }
                ret = KymuxListener::run_task(listener, ctrl) => {
                    if let Err(err) = ret {
                        warn!("KymuxListener task ended with error {err:?}");
                    }
                }
            }
        });

        Ok(Self {
            stop_tx,
            join_handle,
        })
    }

    pub(crate) async fn stop(self) -> Result<()> {
        let ret = self.stop_tx.send(());
        if ret.is_err() {
            warn!("KymuxListener already stopped");
        }

        self.join_handle.await.map_err(|err| {
            warn!("KymuxListener panicked: {err:?}");
            Error::ThreadPanicked
        })?;

        Ok(())
    }

    async fn run_task(
        listener: kymux::ConnectionListener,
        ctrl: web::Data<Mutex<Controller>>,
    ) -> Result<()> {
        let connecting = listener.accept().await?;
        let connection = connecting.complete_connection().await?;

        let mut ctrl = ctrl.lock().await;
        ctrl.kymux = Some(connection);

        Ok(())
    }
}

async fn start_kymux(ctrl: web::Data<Mutex<Controller>>) -> Result<u16> {
    // Load TLS config
    let tls_config = {
        let ctrl = ctrl.lock().await;
        ctrl.tls_config.clone()
    };

    let certificate = rustls::Certificate(tls_config.certificate.clone());
    let private_key = rustls::PrivateKey(tls_config.private_key.clone());

    let mut certs_store = rustls::RootCertStore::empty();
    certs_store.add(&certificate).map_err(|err| {
        warn!("Failed to crate RootCertStore: {err:?}");
        Error::TlsInvalidCertificate { source: err }
    })?;

    // Start connection listener
    let listening_port = LISTENING_PORT;
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listening_port);

    info!("kymux: Listening to {addr}");
    let config = kymux::ServerConfig::new(addr, vec![certificate.clone()], private_key);

    let listener = kymux::ConnectionListener::new(config).await?;

    // Start Listen task
    let listener = KymuxListener::new(listener, ctrl.clone()).await?;

    let mut ctrl = ctrl.lock().await;
    ctrl.kymux_listener = Some(listener);

    Ok(listening_port)
}

async fn start_video(
    process_factory: &mut ProcessFactory,
    uri: String,
    codec: VideoCodec,
    yuv444: bool,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new("Video");

    handler
        .kymux_start_video(process_factory, uri, codec, yuv444)
        .await?;

    Ok(handler)
}

async fn start_audio(
    process_factory: &mut ProcessFactory,
    uri: String,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new("Audio");

    handler.kymux_start_audio(process_factory, uri).await?;

    Ok(handler)
}

async fn start_input(
    process_factory: &mut ProcessFactory,
    uri: String,
) -> Result<inputservice::Handler> {
    let mut inputservice = inputservice::Handler::new();

    inputservice.start_kymux(process_factory, uri).await?;

    Ok(inputservice)
}

pub(crate) struct StartSessionResponse {
    pub(crate) avservice_video: avservice::Handler,
    pub(crate) video_endpoint: u64,

    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) audio_endpoint: Option<u64>,

    pub(crate) inputservice: Option<inputservice::Handler>,
    pub(crate) input_endpoint: Option<u64>,
}

async fn start_session(
    ctrl: &mut Controller,
    request: requests::StartSession,
) -> Result<StartSessionResponse> {
    let Some(kymux) = ctrl.kymux.as_mut() else {
        warn!("Kymux not connected");
        return Err(Error::KymuxNotConnected);
    };

    // AV Server
    let video_endpoint = kymux
        .register_endpoint(kymux::StreamType::AudioVideo, kymux::StreamDirection::Uni)
        .await?;

    info!("Start Video avservice. Endpoint: {video_endpoint:X}");
    let video_uri = kymux.get_uri_for_endpoint(video_endpoint)?;

    let video_codec = request.video_codec.as_str();
    let video_codec = match video_codec {
        "h264" => VideoCodec::H264,
        "hevc" => VideoCodec::HEVC,
        "av1" => VideoCodec::AV1,
        _ => {
            error!("Unexpected video codec {video_codec}");
            return Err(Error::InvalidVideoCodec {
                codec: video_codec.into(),
            });
        }
    };

    let avservice_video = start_video(
        &mut ctrl.process_factory,
        video_uri,
        video_codec,
        request.yuv444,
    )
    .await?;

    // Audio server
    let (audio_endpoint, avservice_audio) = if request.audio {
        let audio_endpoint = kymux
            .register_endpoint(kymux::StreamType::AudioVideo, kymux::StreamDirection::Uni)
            .await?;

        info!("Start Audio avservice. Endpoint: {audio_endpoint:X}");

        let audio_uri = kymux.get_uri_for_endpoint(audio_endpoint)?;

        let avservice_audio = start_audio(&mut ctrl.process_factory, audio_uri).await?;
        (Some(audio_endpoint), Some(avservice_audio))
    } else {
        warn!("Audio disabled");
        (None, None)
    };

    // Input server
    let (input_endpoint, inputservice) = if request.inputs {
        let input_endpoint = kymux
            .register_endpoint(kymux::StreamType::Input, kymux::StreamDirection::Bi)
            .await?;

        info!("Start Inputservice. Endpoint: {input_endpoint:X}");
        let uri = kymux.get_uri_for_endpoint(input_endpoint)?;

        let inputservice = start_input(&mut ctrl.process_factory, uri).await?;
        (Some(input_endpoint), Some(inputservice))
    } else {
        warn!("Input server disabled");
        (None, None)
    };

    Ok(StartSessionResponse {
        avservice_video,
        video_endpoint,
        avservice_audio,
        audio_endpoint,
        inputservice,
        input_endpoint,
    })
}
