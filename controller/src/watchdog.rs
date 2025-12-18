use std::sync::Arc;

use libkypc::process::ProcessFactory;

use crate::avservice;
use crate::{Result, StateMachineProxy};

pub(crate) struct Watchdog {
    avservice: avservice::Handler,
}

impl Watchdog {
    pub(crate) async fn new(
        process_factory: &Arc<ProcessFactory>,
        statemachine_proxy: StateMachineProxy,
    ) -> Result<(Self, Vec<avservice_types::Display>)> {
        // Watchdog is global, not tied to any user session
        let mut avservice = avservice::Handler::new(
            avservice::HandlerType::EnumerateDisplays,
            None,
            statemachine_proxy.clone(),
        );

        let displays = avservice.enumerate_displays(process_factory).await?;

        Ok((Self { avservice }, displays))
    }

    pub(crate) async fn stop(mut self) -> Result<()> {
        self.avservice.stop().await?;

        Ok(())
    }
}
