pub(crate) mod requests;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

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

use requests::KymuxTransportBackend;

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
        info!("Waiting for Kymux connection");
        let connection = listener.accept().await?;

        let mut ctrl = ctrl.lock().await;
        ctrl.kymux = Some(connection);

        Ok(())
    }
}

async fn start_kymux(
    ctrl: web::Data<Mutex<Controller>>,
    backend: KymuxTransportBackend,
    public_port: u16,
) -> Result<requests::StartKymuxResponse> {
    // Drop existing kymux instances
    {
        let mut ctrl = ctrl.lock().await;

        crate::controller::stop_kymux(ctrl.kymux_listener.take(), ctrl.kymux.take()).await?;

        // HACK: Kymux doesn't release its UDP Socket on stop. It is done after
        // some time in background tasks.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Load TLS config
    let tls_config = {
        let ctrl = ctrl.lock().await;
        ctrl.tls_config.clone()
    };

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), LISTENING_PORT);

    // Start Listen task
    let conn_listener = match backend {
        KymuxTransportBackend::Quic => {
            info!("Start Quic listener");

            let certificate = rustls::Certificate(tls_config.certificate.clone());
            let private_key = rustls::PrivateKey(tls_config.private_key.clone());

            let config = kymux::ServerConfig::Quic {
                addr,
                certificate,
                private_key,
            };
            kymux::ConnectionListener::new(config).await?
        }
    };

    // Start Listen task
    let listener = KymuxListener::new(conn_listener, ctrl.clone()).await?;

    let mut ctrl = ctrl.lock().await;
    ctrl.kymux_listener = Some(listener);

    Ok(requests::StartKymuxResponse { port: public_port })
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
    service_config: inputservice_types::ServiceConfig,
    uri: String,
) -> Result<inputservice::Handler> {
    let mut inputservice = inputservice::Handler::new();

    inputservice
        .start_kymux(process_factory, host_config, service_config, uri)
        .await?;

    Ok(inputservice)
}

pub(crate) struct StartSessionResponse {
    pub(crate) avservice_video: Option<avservice::Handler>,
    pub(crate) video_endpoint: Option<u16>,

    pub(crate) avservice_audio: Option<avservice::Handler>,
    pub(crate) audio_endpoint: Option<u16>,

    pub(crate) inputservice: Option<inputservice::Handler>,
    pub(crate) input_endpoint: Option<u16>,
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

        let (video_endpoint, video_uri) = kymux
            .register_video_endpoint_with_ipc_forward(None, video_protocol)
            .await?;

        info!("Start Video avservice. Protocol: {video_protocol:?}, Endpoint: {video_endpoint:X}");

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
        let (audio_endpoint, audio_uri) =
            kymux.register_audio_endpoint_with_ipc_forward(None).await?;

        info!("Start Audio avservice. Endpoint: {audio_endpoint:X}");

        let avservice_audio = start_audio(&ctrl.process_factory, audio_uri).await?;
        (Some(audio_endpoint), Some(avservice_audio))
    } else {
        warn!("Audio disabled");
        (None, None)
    };

    // Input server
    let (input_endpoint, inputservice) = if let Some(inputs_request) = request.inputs {
        let (input_endpoint, uri) = kymux.register_input_endpoint_with_ipc_forward(None).await?;

        info!("Start Inputservice. Endpoint: {input_endpoint:X}");

        let host_config = inputservice_types::HostConfig {
            display_list: ctrl.displays.convert(),
        };

        let service_config = inputservice_types::ServiceConfig {
            cursor_format: inputs_request.cursor_format,
        };

        let inputservice =
            start_input(&ctrl.process_factory, host_config, service_config, uri).await?;
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
