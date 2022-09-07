use log::info;
use tokio::process::{Child, Command};

use crate::error::{Error, Result};
use crate::utils;

pub struct Handler {
    child: Option<Child>,
}

impl Handler {
    pub fn new() -> Self {
        Self { child: None }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }

    pub async fn start(
        &mut self,
        ip: &str,
        port: u16,
        yuv444: bool,
        sdp_file: Option<String>,
    ) -> Result<()> {
        if self.is_running() {
            return Err(Error::StreamerStarted);
        }

        // Build mandatory parameters
        let script_path = utils::get_resource_path("txproto_stream_video.lua");
        let mut params = format!(
            "-s {} {} {} {}",
            script_path.to_str().ok_or(Error::Rejected)?,
            ip,
            port,
            yuv444,
        );

        if let Some(sdp_file) = sdp_file {
            params += &format!(" {}", &sdp_file);
        };

        let txproto_path = utils::get_executable_path("txproto");

        info!("Run {:?} {}", txproto_path, params);

        let child = Command::new(txproto_path)
            .args(params.split(' '))
            .spawn()
            .expect("spawn() failed");
        self.child = Some(child);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        match self.child.as_mut() {
            Some(child) => {
                child.kill().await.expect("kill() failed");

                let status = child.wait().await.expect("wait() failed");
                info!("Streamer exited with: {}", status);
                Ok(())
            }
            None => Err(Error::StreamerStopped),
        }
    }
}
