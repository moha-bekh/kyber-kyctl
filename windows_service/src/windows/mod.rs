mod error;
mod process;

use std::env::current_exe;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use log::{info, warn, LevelFilter};
use log4rs::append::file::FileAppender;
use log4rs::config::{Appender, Config, Root};
use windows_sys::Win32::{Foundation::*, System::Threading::*};
use windows_service::{
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
};

use error::{Error, Result};

windows_service::define_windows_service!(ffi_service_main, service_handler);

const SERVICE_NAME: &str = "kyber";

fn start_subprocess(controller_path: &str) -> Result<HANDLE> {
    unsafe {
        let token = process::get_subprocess_token()?;
        process::start_subprocess(
            controller_path,
            token,
        )
    }
}

#[allow(non_snake_case)]
fn service_main(stop: Arc<AtomicBool>) -> Result<()> {
    let exe_path = if let Ok(mut exe_path) = current_exe() {
        exe_path.pop();
        exe_path
    } else {
        warn!("Couldn't get exe path");
        return Err(Error::Rejected);
    };

    let controller_path = Path::new(&exe_path).join("controller.exe");
    let controller_path = controller_path.into_os_string().into_string().unwrap();

    let mut process = start_subprocess(&controller_path)?;

    while !stop.load(Ordering::Relaxed) {
        let ret = unsafe { WaitForSingleObject(process, 1000) };
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
                process = start_subprocess(&controller_path)?;
                info!("Process started again");
            }
            WAIT_TIMEOUT => {}
            _ => {
                warn!("WaitForSingleObject() unexpected value {}", ret);
                break;
            }
        }
    }

    if stop.load(Ordering::Relaxed) {
        info!("Service stop requested, kill child");
        unsafe { TerminateProcess(process, 0) };
        unsafe { WaitForSingleObject(process, 1000) };
        info!("Child killed");
    }

    unsafe { CloseHandle(process) };

    Ok(())
}

fn notify_state(
    status_handle: &ServiceStatusHandle,
    current_state: ServiceState,
) -> std::result::Result<(), windows_service::Error> {
    let status = ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN | ServiceControlAccept::POWER_EVENT | ServiceControlAccept::PRESHUTDOWN | ServiceControlAccept::SESSION_CHANGE,
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
            },
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
    let logpath = Path::new(&exe_path).join("service.log");

    let logfile = FileAppender::builder().append(true).build(logpath).unwrap();

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(
            Root::builder()
                .appender("logfile")
                .build(LevelFilter::Trace),
        )
        .unwrap();

    log4rs::init_config(config).unwrap();

    windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main)?;
    Ok(())
}
