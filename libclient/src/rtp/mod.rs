pub mod host_service;
pub mod player;

use crate::input_service;
use crate::{ClientConfig, MessageProxy, Result};

pub use player::Player;
const RTP_PORT: u16 = 30000;

pub(crate) struct StartResult {
    pub(crate) player: Player,
    pub(crate) input_service: Option<input_service::Client>,
}

pub(crate) async fn start(config: &ClientConfig, msg_proxy: MessageProxy) -> Result<StartResult> {
    let local_ip = host_service::hello(&config.streamer_ip).await?;

    let sdp_param = host_service::generate_sdp(&config.streamer_ip, config.yuv444).await?;

    let mut player = Player::new(config.window_handle, msg_proxy.clone())?;
    player.play(&local_ip, RTP_PORT, sdp_param, config.audio)?;

    host_service::start_stream(
        &config.streamer_ip,
        &local_ip,
        RTP_PORT,
        config.yuv444,
        config.audio,
        config.inputs,
    )
    .await?;

    let input_service = if config.inputs {
        let ip = config.streamer_ip.clone();

        let create_stream_cb =
            |_target, msg_sink| libkynput_rs::tcp::new_client(ip.as_str(), msg_sink);

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
