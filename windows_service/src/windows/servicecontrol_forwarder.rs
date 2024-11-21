use std::sync::mpsc::{self, TryRecvError};

use windows_service::service::SessionChangeParam;
use windows_sys::Win32::Foundation::HANDLE;

use super::{Error, Event, Result};

pub(crate) struct Sender {
    tx: mpsc::Sender<SessionChangeParam>,
    event: Event,
}

impl Sender {
    pub(crate) fn send(&self, param: SessionChangeParam) -> Result<()> {
        self.tx.send(param).map_err(|_| Error::MpscChannelClosed {
            channel_name: self.event.name().into(),
        })?;

        self.event.set()?;

        Ok(())
    }
}

pub(crate) struct Receiver {
    rx: mpsc::Receiver<SessionChangeParam>,
    event: Event,
}

impl Receiver {
    pub(crate) fn handle(&self) -> HANDLE {
        self.event.handle()
    }

    pub(crate) fn recv(&self) -> Result<SessionChangeParam> {
        self.rx.try_recv().map_err(|err| match err {
            TryRecvError::Empty => Error::MpscChannelEmpty,
            TryRecvError::Disconnected => Error::MpscChannelClosed {
                channel_name: self.event.name().into(),
            },
        })
    }
}

pub(crate) fn create() -> Result<(Sender, Receiver)> {
    let event = Event::new("SessionNotif", false)?;
    let (tx, rx) = mpsc::channel();

    Ok((
        Sender {
            tx,
            event: event.clone(),
        },
        Receiver { rx, event },
    ))
}
