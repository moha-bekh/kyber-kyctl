use std::env;
use std::io;

use log::{info, warn};
use tokio::fs;
use tokio::time::{delay_for, Duration};

use crate::error::{Error, Result};
use crate::input_server;
use crate::streamer;

pub struct Controller {
    streamer: Option<streamer::Handler>,
    input_server: Option<input_server::Handler>,
}

impl Controller {
    pub fn new() -> Self {
        Self {
            streamer: None,
            input_server: None,
        }
    }

    pub async fn generate_sdp(&self, yuv444: bool) -> Result<String> {
        // Run the streamer once to generate the SDP
        let sdp_path = env::temp_dir().join("video.sdp");
        let ret = fs::remove_file(sdp_path.clone()).await;
        if let Err(err) = ret {
            if err.kind() != io::ErrorKind::NotFound {
                warn!("Fail to delete SDP file");
                return Err(Error::IO(err));
            }
        }

        let mut streamer = streamer::Handler::new();
        streamer
            .start(
                "localhost",
                30000,
                yuv444,
                Some(
                    sdp_path
                        .clone()
                        .into_os_string()
                        .into_string()
                        .or(Err(Error::Rejected))?,
                ),
            )
            .await?;

        // Wait for sdp file to be available
        let mut sdp_content = None;

        for _ in 0..50 {
            let ret = fs::read_to_string(&sdp_path).await;
            match ret {
                Ok(file_content) => {
                    info!("Got SDP file");
                    sdp_content = Some(file_content);
                    break;
                }
                Err(_) => {
                    info!("Waiting for SDP file");
                    delay_for(Duration::from_millis(100)).await;
                    continue;
                }
            };
        }

        let ret = streamer.stop().await;
        info!("Streamer stop result: {:?}", ret);

        // Parse SDP content
        for l in sdp_content.ok_or(Error::Rejected)?.lines() {
            if l.starts_with("a=fmtp") {
                return Ok(l.to_string());
            }
        }

        Err(Error::Rejected)
    }

    pub async fn start(&mut self, ip: &str, port: u16, yuv444: bool, inputs: bool) -> Result<()> {
        // Stop streamer if any
        if let Some(streamer) = self.streamer.as_mut() {
            info!("Stop running streamer");

            if streamer.is_running() {
                streamer.stop().await?;
            }
            self.streamer = None
        }

        // Stop input_server if any
        if let Some(input_server) = self.input_server.as_mut() {
            info!("Stop running Input Server");

            if input_server.is_running() {
                input_server.stop().await?;
            }
            self.input_server = None
        }

        // SDP file will contain some required parameters on client side.
        info!("Start. ip: {} - port: {}", ip, port);
        let mut streamer = streamer::Handler::new();
        streamer.start(ip, port, yuv444, None).await?;

        self.streamer = Some(streamer);

        if inputs {
            let mut input_server = input_server::Handler::new();
            input_server.start().await?;

            self.input_server = Some(input_server);
        } else {
            warn!("Input server disabled");
        }

        delay_for(Duration::from_millis(100)).await;
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("Stop");

        match self.streamer.as_mut() {
            Some(streamer) => {
                streamer.stop().await?;
                self.streamer = None
            }
            None => {
                info!("No active streamer");
            }
        }

        match self.input_server.as_mut() {
            Some(input_server) => {
                input_server.stop().await?;
                self.input_server = None
            }
            None => {
                info!("No active Input Server");
            }
        }

        Ok(())
    }
}
