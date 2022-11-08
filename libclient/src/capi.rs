use std::sync::Arc;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};

use log::warn;

use libkynput_rs::Pipeline;

#[allow(non_camel_case_types)]
pub struct libclient_WindowHandle {
    handle: crate::window_handle::WindowHandle,
}

#[allow(non_camel_case_types)]
pub struct libclient_ClientConfig {
    config: crate::ClientConfig,
}

fn result_to_c_int<T>(result: crate::Result<T>) -> c_int {
    match result {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[repr(C)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<log::Level> for LogLevel {
    fn from(t: log::Level) -> Self {
        match t {
            log::Level::Error => Self::Error,
            log::Level::Warn => Self::Warn,
            log::Level::Info => Self::Info,
            log::Level::Debug => Self::Debug,
            log::Level::Trace => Self::Trace,
        }
    }
}

type LogCb = extern "C" fn(
    level: LogLevel,
    target: *const c_char,
    line: *const c_char,
    userdata: *mut c_void,
);

struct Logger {
    cb: LogCb,
    userdata: std::sync::atomic::AtomicPtr<c_void>,
}

impl log::Log for Logger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let line = if let Ok(line) = CString::new(format!("{}", record.args())) {
            line
        } else {
            return;
        };

        let target = if let Ok(target) = CString::new(record.target()) {
            target
        } else {
            return;
        };

        (self.cb)(
            record.level().into(),
            target.as_ptr(),
            line.as_ptr(),
            self.userdata.load(std::sync::atomic::Ordering::Relaxed),
        );
    }

    fn flush(&self) {}
}

#[no_mangle]
pub unsafe extern "C" fn libclient_set_log_cb(cb: LogCb, userdata: *mut c_void) {
    let userdata = std::sync::atomic::AtomicPtr::new(userdata);
    let logger = Box::new(Logger { cb, userdata });

    if log::set_boxed_logger(logger).is_err() {
        return;
    }

    log::set_max_level(log::LevelFilter::Trace);
}

#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_x11(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::window_handle::WindowHandle::Xlib(handle),
    }))
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_ns_view(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::window_handle::WindowHandle::AppKit(handle),
    }))
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_hwnd(
    hwnd: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::window_handle::WindowHandle::Win32(hwnd),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_config_default(
    ip: *const c_char,
    pipeline: *mut libkynput_sys::libkynput_Pipeline,
    window_handle: *mut libclient_WindowHandle,
    out_config: *mut *mut libclient_ClientConfig,
) -> c_int {
    let pipeline = Pipeline::from_ptr(pipeline);
    let window_handle = Box::from_raw(window_handle);

    let ip = if let Ok(ip) = CStr::from_ptr(ip).to_str() {
        ip
    } else {
        warn!("Fail to convert client IP");
        return -1;
    };

    let config = crate::ClientConfig {
        streamer_ip: ip.to_string(),
        input_pipeline: pipeline,
        window_handle: window_handle.handle,
        yuv444: false,
        inputs: true,
        audio: false,
    };

    *out_config = Box::into_raw(Box::new(libclient_ClientConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_config_set_yuv444(
    config: *mut libclient_ClientConfig,
    yuv444: bool,
) {
    (*config).config.yuv444 = yuv444;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_config_set_inputs(
    config: *mut libclient_ClientConfig,
    inputs: bool,
) {
    (*config).config.inputs = inputs;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_config_set_audio(
    config: *mut libclient_ClientConfig,
    audio: bool,
) {
    (*config).config.audio = audio;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_config_unref(cfg: *mut libclient_ClientConfig) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[allow(non_camel_case_types)]
pub struct libclient_Client {
    client: Arc<crate::Client>,
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_new(
    config: *mut libclient_ClientConfig,
) -> *mut libclient_Client {
    let config = Box::from_raw(config);

    Box::into_raw(Box::new(libclient_Client {
        client: Arc::new(crate::Client::new(config.config)),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_start(client: *mut libclient_Client) -> c_int {
    let ret = (*client).client.start();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_stop(client: *mut libclient_Client) -> c_int {
    let ret = (*client).client.stop();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_ref(
    client: *mut libclient_Client,
) -> *mut libclient_Client {
    Box::into_raw(Box::new(libclient_Client {
        client: (*client).client.clone(),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_unref(client: *mut libclient_Client) {
    let _ = Box::from_raw(client);
}

#[allow(non_camel_case_types)]
pub type Event = super::Event;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct libclient_EventHandler {
    on_event: Option<extern "C" fn(event: *const Event, userdata: *mut c_void)>,
    userdata: *mut c_void,
}

unsafe impl Send for libclient_EventHandler {}
unsafe impl Sync for libclient_EventHandler {}

impl crate::event_manager::EventSink for libclient_EventHandler {
    fn on_event(&self, event: &crate::Event) {
        if let Some(on_event) = self.on_event {
            on_event(event, self.userdata);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_set_event_handler(
    client: *mut libclient_Client,
    handler: libclient_EventHandler,
) -> c_int {
    if let Ok(mut event_manager) = (*client).client.event_manager.lock() {
        event_manager.add_event_sink(Box::new(handler));
        0
    } else {
        warn!("Fail to get EventManager");
        -1
    }
}
