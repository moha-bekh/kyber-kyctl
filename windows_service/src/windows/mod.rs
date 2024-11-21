mod config;
mod error;
mod event;
mod handle;
mod monitors;
mod process;
mod sas;
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
        ServiceType,
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

windows_service::define_windows_service!(ffi_service_main, service_handler);

const SERVICE_NAME: &str = "kyber";

fn start_subprocess(controller_path: &str, inherited: &[&Event]) -> Result<Handle> {
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

    let Some(session_id) = session_id else {
        warn!("No session attached (WTSGetActiveConsoleSessionId() failed");
        return Err(Error::Rejected);
    };

    unsafe {
        let token = process::get_subprocess_token(session_id)?;
        process::start_subprocess(controller_path, token, inherited)
    }
}

#[allow(non_snake_case)]
fn service_main(stop: Arc<AtomicBool>) -> Result<()> {
    // Load configuration
    let config = config::load()?;

    // Plug monitor if required
    let _device_interface = if monitors::must_plug_monitor(&config) {
        monitors::start_kidd(&config)?
    } else {
        None
    };

    // Create SasEvent
    let sas_event = Event::new("KYBER_SAS_EVENT", true)?;

    // Spawn process
    let exe_path = if let Ok(mut exe_path) = current_exe() {
        exe_path.pop();
        exe_path
    } else {
        warn!("Couldn't get exe path");
        return Err(Error::Rejected);
    };

    let controller_path = Path::new(&exe_path).join("controller.exe");
    let controller_path = controller_path.into_os_string().into_string().unwrap();

    let inherited_events = vec![&sas_event];
    let mut process = start_subprocess(&controller_path, &inherited_events)?;

    const WAIT_OBJECT_1: u32 = WAIT_OBJECT_0 + 1;

    while !stop.load(Ordering::Relaxed) {
        let events = [process.handle(), sas_event.handle()];

        let ret =
            unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), FALSE, 1000) };

        match ret {
            WAIT_OBJECT_0 => {
                // On log out, all the process from the current session are
                // stopped. Because we attach controller.exe to the current
                // interactive session, it is killed. On restart, controller.exe
                // will be attached to the new session login screen.
                //
                // The current service isn't killed because it is attached to
                // the special Session 0.
                info!("Process stopped");
                process = start_subprocess(&controller_path, &inherited_events)?;
                info!("Process started again");
            }
            WAIT_OBJECT_1 => {
                unsafe { ResetEvent(sas_event.handle()) };
                sas::send();
            }
            WAIT_TIMEOUT => {}
            WAIT_FAILED => {
                let err = last_win32_error("WaitForMultipleObjects", std::file!(), std::line!());
                warn!("WaitForMultipleObjects() failed: {err:?}");
            }
            _ => {
                warn!("WaitForMultipleObjects() unexpected value {}", ret);
                break;
            }
        }
    }

    if stop.load(Ordering::Relaxed) {
        info!("Service stop requested, kill child");
        unsafe { TerminateProcess(process.handle(), 0) };
        unsafe { WaitForSingleObject(process.handle(), 1000) };
        info!("Child killed");
    }

    Ok(())
}

fn notify_state(
    status_handle: &ServiceStatusHandle,
    current_state: ServiceState,
) -> std::result::Result<(), windows_service::Error> {
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

    status_handle.set_service_status(status)
}

fn service_handler(_arguments: Vec<OsString>) {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_evt_handler = stop.clone();

    // Register Control callback
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop => {
                info!("Service stop request received");
                stop_evt_handler.store(true, Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::SessionChange(param) => {
                info!("Session changed: {param:?}");
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => {
                info!("Got event: {control_event:?}");
                ServiceControlHandlerResult::NoError
            }
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler).unwrap();

    // Notify that the service is running
    notify_state(&status_handle, ServiceState::Running).unwrap();
    info!("Service started");

    // Run real main function
    let ret = service_main(stop);
    if ret.is_err() {
        warn!("Service exited with error: '{:?}'", ret);
    }

    info!("Service stopped");
    notify_state(&status_handle, ServiceState::Stopped).unwrap();
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
