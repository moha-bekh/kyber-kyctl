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

    pub async fn start(&mut self, script: &str, args: Option<Vec<String>>) -> Result<()> {
        if self.is_running() {
            return Err(Error::StreamerStarted);
        }

        // Build mandatory parameters
        let txproto_path = utils::get_executable_path("avserver");
        let script_path = utils::get_resource_path(&script);

        let mut params: Vec<String> = vec![
            script_path.into_os_string().into_string().unwrap(),
        ];

        if let Some(mut args) = args {
            params.append(&mut args);
        };

        info!("Run {:?} {:?}", txproto_path, params);

        let child = Command::new(txproto_path)
            .args(params)
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
