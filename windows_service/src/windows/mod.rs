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

use log::{info, warn, LevelFilter};
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
    sc_forwarder_rx: servicecontrol_forwarder::Receiver,
}

impl ServiceContext {
    fn new(
        stop: Arc<AtomicBool>,
        sc_forwarder_rx: servicecontrol_forwarder::Receiver,
    ) -> Result<Self> {
        let config = config::load()?;

        // Plug monitor if required
        let _kidd_interface = if monitors::must_plug_monitor(&config) {
            monitors::start_kidd(&config)?
        } else {
            None
        };

        Ok(Self {
            stop,
            _kidd_interface,
            sas_event: Event::new("KYBER_SAS_EVENT", true)?,
            controller_path: Self::get_controller_path()?,
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

    fn start_controller(&self, session_id: u32, inherited: &[&Event]) -> Result<Process> {
        info!("Start controller.exe in session {session_id}");
        Process::new(&self.controller_path, session_id, inherited)
    }

    fn get_controller_path() -> Result<String> {
        let exe_path = if let Ok(mut exe_path) = current_exe() {
            exe_path.pop();
            exe_path
        } else {
            warn!("Couldn't get exe path");
            return Err(Error::Rejected);
        };

        let controller_path = Path::new(&exe_path).join("controller.exe");
        controller_path.into_os_string().into_string().map_err(|_| {
            warn!("Controller path contains invalid Unicode data");
            Error::InvalidUnicodeData
        })
    }

    fn run(&self) -> Result<()> {
        let inherited_events = vec![&self.sas_event];
        let mut restart_controller = false;

        let mut active_session_id = Self::find_active_console_session();

        let mut process = match active_session_id {
            Some(session_id) => {
                info!("Session {session_id} already active");
                Some(self.start_controller(session_id, &inherited_events)?)
            }
            None => {
                info!("No active Console session found");
                None
            }
        };

        const WAIT_OBJECT_1: u32 = WAIT_OBJECT_0 + 1;
        const WAIT_OBJECT_2: u32 = WAIT_OBJECT_0 + 2;

        const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
        const RESTART_CONTROLLER_TIMEOUT: Duration = Duration::from_millis(500);

        let mut timeout = DEFAULT_TIMEOUT;

        while !self.stop.load(Ordering::Relaxed) {
            let events = if let Some(process) = &process {
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
                    timeout.as_millis() as _,
                )
            };

            match ret {
                WAIT_OBJECT_0 => {
                    unsafe { ResetEvent(self.sas_event.handle()) };
                    sas::send();
                }
                WAIT_OBJECT_1 => {
                    while let Ok(param) = self.sc_forwarder_rx.recv() {
                        match param {
                            SessionChangeParam {
                                reason: SessionChangeReason::ConsoleConnect,
                                notification: SessionNotification { session_id, .. },
                            } => {
                                if let Some(process) = process.take() {
                                    warn!("Console connected. Kill existing controller.exe");
                                    let ret = process.kill();
                                    if let Err(err) = ret {
                                        warn!("Failed to kill controller.exe: {err:?}");
                                    }
                                }

                                process =
                                    Some(self.start_controller(session_id, &inherited_events)?);
                                active_session_id = Some(session_id);

                                restart_controller = false;
                                timeout = DEFAULT_TIMEOUT;
                            }
                            SessionChangeParam {
                                reason: SessionChangeReason::ConsoleDisconnect,
                                notification: SessionNotification { session_id, .. },
                            } => {
                                match active_session_id.take() {
                                    Some(active_session_id) => {
                                        if active_session_id != session_id {
                                            warn!("ConsoleDisconnect received on session {session_id} while we though that the active one was session {active_session_id}")
                                        }
                                    }
                                    None => {
                                        warn!("ConsoleDisconnect received on session {session_id} while we though that no session was active")
                                    }
                                }

                                restart_controller = false;
                                timeout = DEFAULT_TIMEOUT;
                            }
                            _ => {}
                        }
                    }
                }
                WAIT_OBJECT_2 => {
                    // On log out, all the process from the current session are
                    // stopped. Because we attach controller.exe to the current
                    // interactive session, it is killed. On restart, controller.exe
                    // will be attached to the new session login screen.
                    //
                    // The current service isn't killed because it is attached to
                    // the special Session 0.
                    info!("controller.exe stopped");

                    if active_session_id.is_some() {
                        restart_controller = true;
                        process = None;
                        timeout = RESTART_CONTROLLER_TIMEOUT;
                    }
                }
                WAIT_TIMEOUT => {
                    if restart_controller {
                        if let Some(session_id) = &active_session_id {
                            process = Some(self.start_controller(*session_id, &inherited_events)?);
                        } else {
                            info!(
                                "controller.exe restart requested, but there is no active session"
                            );
                        }

                        restart_controller = false;
                        timeout = DEFAULT_TIMEOUT;
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

        if self.stop.load(Ordering::Relaxed) {
            if let Some(process) = process.take() {
                info!("Service stop requested, kill controller.exe");
                let ret = process.kill();
                if let Err(err) = ret {
                    warn!("Failed to kill controller.exe: {err:?}");
                }

                info!("controller.exe killed");
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
    let ctx = ServiceContext::new(stop, sc_forwarder_rx)?;
    let ret = ctx.run();
    if ret.is_err() {
        warn!("Service exited with error: '{:?}'", ret);
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
    let logpath = Path::new(&exe_path).join("log").join("service.log");

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
