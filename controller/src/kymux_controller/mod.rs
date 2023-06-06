pub(crate) mod requests;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use actix_web::web;
use log::{info, warn};
use tokio::sync::{oneshot, Mutex};
use tokio::task::{self, JoinHandle};

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::error::{Error, Result};
use crate::inputservice;
use crate::utils::TypeConvert;
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

async fn start_kymux(ctrl: web::Data<Mutex<Controller>>, public_port: u16) -> Result<u16> {
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

    // Start connection listener.
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), LISTENING_PORT);

    info!("kymux: Listening to {addr}");
    let config = kymux::ServerConfig::new(addr, vec![certificate.clone()], private_key);

    let listener = kymux::ConnectionListener::new(config).await?;

    // Start Listen task
    let listener = KymuxListener::new(listener, ctrl.clone()).await?;

    let mut ctrl = ctrl.lock().await;
    ctrl.kymux_listener = Some(listener);

    Ok(public_port)
}

async fn start_video(
    process_factory: &Arc<ProcessFactory>,
    display_id: u32,
    uri: String,
    bitrate: u32,
    codec: avservice_types::VideoCodec,
    yuv444: bool,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new("Video");

    handler
        .kymux_start_video(process_factory, display_id, uri, bitrate, codec, yuv444)
        .await?;

    Ok(handler)
}

async fn start_audio(
    process_factory: &Arc<ProcessFactory>,
    uri: String,
) -> Result<avservice::Handler> {
    let mut handler = avservice::Handler::new("Audio");

    handler.kymux_start_audio(process_factory, uri).await?;

    Ok(handler)
}

async fn start_input(
    process_factory: &Arc<ProcessFactory>,
    host_config: inputservice_types::HostConfig,
    uri: String,
) -> Result<inputservice::Handler> {
    let mut inputservice = inputservice::Handler::new();

    inputservice
        .start_kymux(process_factory, host_config, uri)
        .await?;

    Ok(inputservice)
}

pub(crate) struct StartSessionResponse {
    pub(crate) avservice_video: Option<avservice::Handler>,
    pub(crate) video_endpoint: Option<u64>,

    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) audio_endpoint: Option<u64>,

    pub(crate) inputservice: Option<inputservice::Handler>,
    pub(crate) input_endpoint: Option<u64>,
}

async fn start_session(
    ctrl: &mut Controller,
    request: requests::StartRequest,
) -> Result<StartSessionResponse> {
    let Some(kymux) = ctrl.kymux.as_mut() else {
        warn!("Kymux not connected");
        return Err(Error::KymuxNotConnected);
    };

    let (video_endpoint, avservice_video) = if let Some(video_request) = request.video {
        // Video Server
        let video_protocol = request
            .kymux_config
            .video_protocol
            .unwrap_or(kymux::VideoProtocol::Reliable);

        let video_endpoint = kymux
            .register_endpoint(kymux::StreamType::Video(video_protocol))
            .await?;

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {video_endpoint:X}");
        let video_uri = kymux.get_uri_for_endpoint(video_endpoint)?;

        let avservice_video = start_video(
            &ctrl.process_factory,
            video_request.display_id,
            video_uri,
            request.bitrate,
            video_request.codec,
            video_request.yuv444,
        )
        .await?;

        (Some(video_endpoint), Some(avservice_video))
    } else {
        (None, None)
    };

    // Audio server
    let (audio_endpoint, avservice_audio) = if request.audio {
        let audio_endpoint = kymux.register_endpoint(kymux::StreamType::Audio).await?;

        info!("Start Audio avservice. Endpoint: {audio_endpoint:X}");

        let audio_uri = kymux.get_uri_for_endpoint(audio_endpoint)?;

        let avservice_audio = start_audio(&ctrl.process_factory, audio_uri).await?;
        (Some(audio_endpoint), Some(avservice_audio))
    } else {
        warn!("Audio disabled");
        (None, None)
    };

    // Input server
    let (input_endpoint, inputservice) = if request.inputs {
        let input_endpoint = kymux.register_endpoint(kymux::StreamType::Input).await?;

        info!("Start Inputservice. Endpoint: {input_endpoint:X}");
        let uri = kymux.get_uri_for_endpoint(input_endpoint)?;

        let host_config = inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        let inputservice = start_input(&ctrl.process_factory, host_config, uri).await?;
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
