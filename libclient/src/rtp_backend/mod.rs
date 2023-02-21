pub mod host_service;
pub mod player;

use std::net::SocketAddr;

use crate::http::HttpClient;
use crate::input_service;
use crate::{ClientConfig, MessageProxy, Result};

pub use player::Player;
const RTP_PORT: u16 = 30000;

pub(crate) struct StartResult {
    pub(crate) player: Player,
    pub(crate) input_service: Option<input_service::Client>,
}

pub(crate) async fn start(
    config: &ClientConfig,
    addr: &SocketAddr,
    client: &HttpClient,
    msg_proxy: MessageProxy,
) -> Result<StartResult> {
    let local_ip = host_service::hello(client).await?;

    let sdp_param = host_service::generate_sdp(client, config.yuv444).await?;

    let mut player = Player::new(config.window_handle, msg_proxy.clone())?;
    player.play(&local_ip, RTP_PORT, sdp_param, config.audio)?;

    host_service::start_stream(
        client,
        &local_ip,
        RTP_PORT,
        config.yuv444,
        config.audio,
        config.inputs,
    )
    .await?;

    let input_service = if config.inputs {
        let create_stream_cb =
            |_target, msg_sink| libkynput_rs::tcp::new_client(&addr.ip().to_string(), msg_sink);

        let input_service = input_service::Client::new(
            msg_proxy,
            config.input_pipeline.get_input_sink(),
            create_stream_cb,
        )?;

        Some(input_service)
    } else {
        None
    };

    Ok(StartResult {
        player,
        input_service,
    })
}
