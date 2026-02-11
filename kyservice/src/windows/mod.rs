// Project Kyber: mod.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0
//
// This file is both under dual license: AGPLv3 and a Commercial one.
//
// ----
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

mod config;
mod error;
mod event;
mod handle;
mod monitors;
mod process;
mod sas;
mod servicecontrol_forwarder;
mod utils;

use std::env::current_exe;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{error, info, warn, LevelFilter};
use log4rs::append::file::FileAppender;
use windows_service::{
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType, SessionChangeParam, SessionChangeReason, SessionNotification,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{RemoteDesktop::*, Threading::*},
};

use error::{last_win32_error, Error, Result};
use event::Event;
use handle::Handle;
use process::Process;

windows_service::define_windows_service!(ffi_service_main, service_handler_wrapper);

const SERVICE_NAME: &str = "kyber";

struct ServiceContext {
    stop: Arc<AtomicBool>,
    _kidd_interface: Option<kidd_client::DeviceInterface>,
    sas_event: Event,
    controller_path: String,
    process: Option<Process>,
    restart_controller: bool,
    timeout: Duration,
    active_session_id: Option<u32>,
    sc_forwarder_rx: servicecontrol_forwarder::Receiver,
}

impl ServiceContext {
    const WAIT_OBJECT_1: u32 = WAIT_OBJECT_0 + 1;
    const WAIT_OBJECT_2: u32 = WAIT_OBJECT_0 + 2;

    const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
    const RESTART_CONTROLLER_TIMEOUT: Duration = Duration::from_millis(500);

    fn new(
        stop: Arc<AtomicBool>,
        sc_forwarder_rx: servicecontrol_forwarder::Receiver,
    ) -> Result<Self> {
        let config = config::load()?;

        // Plug monitor if required
        let _kidd_interface = if config.kidd.enabled {
            match monitors::start_kidd(&config) {
                Ok(interface) => interface,
                Err(e) => {
                    error!("Failed to start Kidd: {:?}. Service will continue without virtual monitors.", e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            stop,
            _kidd_interface,
            sas_event: Event::new("KYBER_SAS_EVENT", true)?,
            controller_path: Self::get_controller_path()?,
            process: None,
            restart_controller: false,
            timeout: Self::DEFAULT_TIMEOUT,
            active_session_id: None,
            sc_forwarder_rx,
        })
    }

    fn find_active_console_session() -> Option<u32> {
        let mut session_id = None;

        // At Windows startup, we can be started before WTSGetActiveConsoleSessionId()
        // returns a valid value. Retry multiple time before starting the process.
        let retry_delay = Duration::from_millis(100);
        let max_retry_count = 100; // 10 seconds

        let mut i = 0;
        while i < max_retry_count {
            let id = unsafe { WTSGetActiveConsoleSessionId() };
            if id != 0xFFFFFFFF {
                session_id = Some(id);
                break;
            } else {
                thread::sleep(retry_delay);
            }

            i += 1;
        }

        session_id
    }

    fn inherited_events(&self) -> Vec<&Event> {
        vec![&self.sas_event]
    }

    fn start_controller(&self, session_id: u32) -> Result<Process> {
        let inherited_events = self.inherited_events();

        info!("Start kycontroller.exe in session {session_id}");
        Process::new(&self.controller_path, session_id, &inherited_events)
    }

    fn get_controller_path() -> Result<String> {
        let exe_path = if let Ok(mut exe_path) = current_exe() {
            exe_path.pop();
            exe_path
        } else {
            warn!("Couldn't get exe path");
            return Err(Error::Rejected);
        };

        let controller_path = Path::new(&exe_path).join("kycontroller.exe");
        controller_path.into_os_string().into_string().map_err(|_| {
            warn!("kycontroller path contains invalid Unicode data");
            Error::InvalidUnicodeData
        })
    }

    fn handle_sas(&self) {
        unsafe { ResetEvent(self.sas_event.handle()) };
        sas::send();
    }

    fn handle_sc_forwarder(&mut self) -> Result<()> {
        while let Ok(param) = self.sc_forwarder_rx.recv() {
            match param {
                SessionChangeParam {
                    reason: SessionChangeReason::ConsoleConnect,
                    notification: SessionNotification { session_id, .. },
                } => {
                    if let Some(process) = self.process.take() {
                        warn!("Console connected. Kill existing kycontroller.exe");
                        let ret = process.kill();
                        if let Err(err) = ret {
                            warn!("Failed to kill kycontroller.exe: {err:?}");
                        }
                    }

                    self.process = Some(self.start_controller(session_id)?);
                    self.active_session_id = Some(session_id);

                    self.restart_controller = false;
                    self.timeout = Self::DEFAULT_TIMEOUT;
                }
                SessionChangeParam {
                    reason: SessionChangeReason::ConsoleDisconnect,
                    notification: SessionNotification { session_id, .. },
                } => {
                    match self.active_session_id.take() {
                        Some(active_session_id) => {
                            if active_session_id != session_id {
                                warn!("ConsoleDisconnect received on session {session_id} while we though that the active one was session {active_session_id}")
                            }
                        }
                        None => {
                            warn!("ConsoleDisconnect received on session {session_id} while we though that no session was active")
                        }
                    }

                    self.restart_controller = false;
                    self.timeout = Self::DEFAULT_TIMEOUT;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn handle_controller_stopped(&mut self) -> Result<()> {
        // On log out, all the process from the current session are
        // stopped. Because we attach kycontroller.exe to the current
        // interactive session, it is killed. On restart, kycontroller.exe
        // will be attached to the new session login screen.
        //
        // The current service isn't killed because it is attached to
        // the special Session 0.
        info!("kycontroller.exe stopped");

        if self.active_session_id.is_some() {
            self.restart_controller = true;
            self.process = None;
            self.timeout = Self::RESTART_CONTROLLER_TIMEOUT;
        }

        Ok(())
    }

    fn handle_timeout(&mut self) -> Result<()> {
        if self.restart_controller {
            if let Some(session_id) = &self.active_session_id {
                self.process = Some(self.start_controller(*session_id)?);
            } else {
                info!("kycontroller.exe restart requested, but there is no active session");
            }

            self.restart_controller = false;
            self.timeout = Self::DEFAULT_TIMEOUT;
        }

        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        self.active_session_id = Self::find_active_console_session();

        self.process = match self.active_session_id {
            Some(session_id) => {
                info!("Session {session_id} already active");
                Some(self.start_controller(session_id)?)
            }
            None => {
                info!("No active Console session found");
                None
            }
        };

        while !self.stop.load(Ordering::Relaxed) {
            let events = if let Some(process) = &self.process {
                vec![
                    self.sas_event.handle(),
                    self.sc_forwarder_rx.handle(),
                    process.handle(),
                ]
            } else {
                vec![self.sas_event.handle(), self.sc_forwarder_rx.handle()]
            };

            let ret = unsafe {
                WaitForMultipleObjects(
                    events.len() as u32,
                    events.as_ptr(),
                    FALSE,
                    self.timeout.as_millis() as _,
                )
            };

            match ret {
                WAIT_OBJECT_0 => {
                    self.handle_sas();
                }
                Self::WAIT_OBJECT_1 => {
                    let ret = self.handle_sc_forwarder();
                    if let Err(err) = ret {
                        warn!("handle_sc_forwarder() failed: {err:?}");
                    }
                }
                Self::WAIT_OBJECT_2 => {
                    let ret = self.handle_controller_stopped();
                    if let Err(err) = ret {
                        warn!("handle_controller_stopped() failed: {err:?}");
                    }
                }
                WAIT_TIMEOUT => {
                    let ret = self.handle_timeout();
                    if let Err(err) = ret {
                        warn!("handle_timeout() failed: {err:?}");
                    }
                }
                WAIT_FAILED => {
                    let err =
                        last_win32_error("WaitForMultipleObjects", std::file!(), std::line!());
                    warn!("WaitForMultipleObjects() failed: {err:?}");
                }
                _ => {
                    warn!("WaitForMultipleObjects() unexpected value {}", ret);
                    break;
                }
            }
        }

        Ok(())
    }
}

fn notify_state(status_handle: &ServiceStatusHandle, current_state: ServiceState) -> Result<()> {
    let status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state,
        controls_accepted: ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::POWER_EVENT
            | ServiceControlAccept::PRESHUTDOWN
            | ServiceControlAccept::SESSION_CHANGE,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };

    status_handle.set_service_status(status)?;

    Ok(())
}

fn service_handler(_arguments: Vec<OsString>) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_evt_handler = stop.clone();

    // Register Control callback
    let (sc_forwarder_tx, sc_forwarder_rx) = servicecontrol_forwarder::create()?;

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                info!("Service stop request received");
                stop_evt_handler.store(true, Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::SessionChange(param) => {
                info!("Session changed: {param:?}");
                let ret = sc_forwarder_tx.send(param);
                if let Err(err) = ret {
                    warn!("Fail to forward SessionChange: {err:?}");
                }

                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => {
                info!("Got event: {control_event:?}");
                ServiceControlHandlerResult::NoError
            }
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

    // Notify that the service is running
    notify_state(&status_handle, ServiceState::Running)?;
    info!("Service started");

    // Run real main function
    let mut ctx = ServiceContext::new(stop, sc_forwarder_rx)?;
    let ret = ctx.run();
    if ret.is_err() {
        warn!("Service exited with error: '{:?}'", ret);
    }

    // Stop any running ky.exe
    if let Some(process) = ctx.process.take() {
        info!("Service stop requested, kill kycontroller.exe");
        let ret = process.kill();
        if let Err(err) = ret {
            warn!("Failed to kill kycontroller.exe: {err:?}");
        }

        info!("kycontroller.exe killed");
    }

    info!("Service stopped");
    notify_state(&status_handle, ServiceState::Stopped)?;

    Ok(())
}

fn service_handler_wrapper(arguments: Vec<OsString>) {
    let ret = service_handler(arguments);
    if let Err(err) = ret {
        warn!("Service handler ended with error {err:?}");
    }
}

pub fn run() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut exe_path = current_exe().unwrap();
    exe_path.pop();
    let logpath = Path::new(&exe_path).join("log").join("kyservice.log");

    let logfile = FileAppender::builder().append(true).build(logpath).unwrap();

    let config = log4rs::config::Config::builder()
        .appender(log4rs::config::Appender::builder().build("logfile", Box::new(logfile)))
        .build(
            log4rs::config::Root::builder()
                .appender("logfile")
                .build(LevelFilter::Trace),
        )
        .unwrap();

    log4rs::init_config(config).unwrap();

    info!(
        "Kyber Windows service. Revision {}",
        env!("VERGEN_GIT_DESCRIBE")
    );

    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}
