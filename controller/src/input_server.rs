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

    pub async fn start(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Err(Error::StreamerStarted);
        }

        info!("Run Input Server");

        let exe_path = utils::get_executable_path("input_server");

        let child = Command::new(exe_path).spawn().expect("spawn() failed");
        self.child = Some(child);

        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        match self.child.as_mut() {
            Some(child) => {
                child.kill()?;
                let status = child.await.expect("wait() failed");
                info!("Input server exited with: {}", status);
                Ok(())
            }
            None => Err(Error::StreamerStopped),
        }
    }
}
