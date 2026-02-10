// Project Kyber: capi.rs
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

use std::sync::Arc;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::AtomicPtr;

use log::{error, warn};
use std::io::BufReader;

use kymux::kyproto;
use kynput_rs::Pipeline;

use crate::metrics::{
    Metric, MetricsJSONProducer, NetworkPing, NetworkStats, VideoMetric, VideoMetricData,
};
use crate::platform::tls::get_resource_path;
use crate::platform::VerifyMode;
use crate::MetricsCallback;
use crate::VideoCodec;

#[allow(non_camel_case_types)]
pub type Display = super::Display;

#[allow(non_camel_case_types)]
pub struct libclient_WindowHandle {
    handle: crate::platform::WindowHandle,
}

#[allow(non_camel_case_types)]
pub struct libclient_ConnectConfig {
    config: crate::ConnectConfig,
}

#[repr(C)]
pub enum libclient_AuthType {
    Basic,
    Jwt,
}

#[allow(non_camel_case_types)]
pub struct libclient_AuthCredentials {
    inner: crate::AuthCredentials,
}

#[allow(non_camel_case_types)]
pub struct libclient_StreamingProtocol {
    protocol: crate::StreamingProtocol,
}

#[allow(non_camel_case_types)]
pub struct libclient_StreamingSessionConfig {
    config: crate::StreamingSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_VideoPlayerConfig {
    config: crate::VideoPlayerConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_VideoSessionConfig {
    config: crate::VideoSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_AudioSessionConfig {
    config: crate::AudioSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct libclient_InputSessionConfig {
    config: crate::InputSessionConfig,
}

fn result_to_c_int<T>(result: crate::Result<T>) -> c_int {
    match result {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "C" fn libclient_init_crypto() {
    crate::init_crypto();
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum libclient_KymuxVideoProtocol {
    Reliable,
    GopStream,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<libclient_KymuxVideoProtocol> for kyproto::VideoProtocol {
    fn from(value: libclient_KymuxVideoProtocol) -> Self {
        match value {
            libclient_KymuxVideoProtocol::Reliable => Self::Reliable,
            libclient_KymuxVideoProtocol::GopStream => Self::GopStream,
            libclient_KymuxVideoProtocol::Unreliable => Self::Unreliable,
            libclient_KymuxVideoProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum libclient_KymuxAudioProtocol {
    Reliable,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<libclient_KymuxAudioProtocol> for kyproto::AudioProtocol {
    fn from(value: libclient_KymuxAudioProtocol) -> Self {
        match value {
            libclient_KymuxAudioProtocol::Reliable => Self::Reliable,
            libclient_KymuxAudioProtocol::Unreliable => Self::Unreliable,
            libclient_KymuxAudioProtocol::UnreliableFec => Self::UnreliableFec,
        }
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

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn libclient_set_android_logger() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("libclient"),
    );
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_set_android_context(
    awindow_handler: *mut c_void,
) -> *mut libclient_WindowHandle {
    log::info!(
        "libclient_windowshandle_set_android_context {:?}",
        awindow_handler
    );
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::Android(awindow_handler as u64),
    }))
}

#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_x11(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::Xlib(handle),
    }))
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_ns_view(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::AppKit(handle),
    }))
}

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_ui_view(
    handle: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::UiKit(handle),
    }))
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub unsafe extern "C" fn libclient_windowshandle_from_hwnd(
    hwnd: u64,
) -> *mut libclient_WindowHandle {
    Box::into_raw(Box::new(libclient_WindowHandle {
        handle: crate::platform::WindowHandle::Win32(hwnd),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_auth_credential_new(
    type_: libclient_AuthType,
    credentials: *const c_char,
) -> *mut libclient_AuthCredentials {
    let Ok(credentials) = CStr::from_ptr(credentials).to_str() else {
        warn!("Fail to convert credentials");
        return ptr::null_mut();
    };

    let credentials = credentials.to_owned();

    let credentials = match type_ {
        libclient_AuthType::Basic => crate::AuthCredentials::Basic {
            username: credentials,
        },
        libclient_AuthType::Jwt => crate::AuthCredentials::Jwt { token: credentials },
    };

    Box::into_raw(Box::new(libclient_AuthCredentials { inner: credentials }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_default(
    host: *const c_char,
    out_config: *mut *mut libclient_ConnectConfig,
) -> c_int {
    let Ok(host) = CStr::from_ptr(host).to_str() else {
        warn!("Fail to convert client Host");
        return -1;
    };

    let config = crate::ConnectConfig {
        host: host.into(),
        port: crate::STREAMER_PORT,
        credentials: crate::AuthCredentials::default(),
        tls_host: None,
        verify_mode: None,
        auto_reconnection: false,
    };

    *out_config = Box::into_raw(Box::new(libclient_ConnectConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_credentials(
    config: *mut libclient_ConnectConfig,
    credentials: *mut libclient_AuthCredentials,
) -> c_int {
    let credentials = Box::from_raw(credentials);
    (*config).config.credentials = credentials.inner;

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_tls_host(
    config: *mut libclient_ConnectConfig,
    tls_host: *const c_char,
) -> c_int {
    let Ok(tls_host) = CStr::from_ptr(tls_host).to_str() else {
        warn!("Fail to convert client TLS host");
        return -1;
    };

    (*config).config.tls_host = Some(tls_host.into());

    0
}

#[no_mangle]
#[deprecated = "use libclient_connect_config_set_tls_root_cert_path"]
pub unsafe extern "C" fn libclient_connect_config_set_tls_root_cert(
    config: *mut libclient_ConnectConfig,
    tls_root_cert: *const c_char,
) -> c_int {
    libclient_connect_config_set_tls_root_cert_path(config, tls_root_cert)
}

/// Set TLS root certificate from a PEM file path.
///
/// This C API is preserved for backward compatibility with Android and iOS clients.
/// Internally converts to `VerifyMode::RootCert` - the old `tls_config` codepath
/// has been removed in favor of the unified `verify_mode` system.
///
/// The path is resolved relative to the executable directory if it's a relative path.
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_tls_root_cert_path(
    config: *mut libclient_ConnectConfig,
    path: *const c_char,
) -> c_int {
    let Ok(path_str) = CStr::from_ptr(path).to_str() else {
        warn!("Fail to convert Root TLS certificate path");
        return -1;
    };

    let resolved_path = get_resource_path(path_str);

    let Ok(file) = std::fs::File::open(&resolved_path) else {
        warn!(
            "Failed to open TLS root certificate file: {:?}",
            resolved_path
        );
        return -1;
    };

    let mut reader = BufReader::new(file);

    // Parse PEM and extract DER bytes
    let root_certificate = match rustls_pemfile::read_one(&mut reader) {
        Ok(Some(rustls_pemfile::Item::X509Certificate(cert))) => cert.to_vec(),
        Ok(_) => {
            error!("{:?} doesn't contain a X509 Certificate", resolved_path);
            return -1;
        }
        Err(e) => {
            error!(
                "Failed to parse TLS certificate {:?}: {:?}",
                resolved_path, e
            );
            return -1;
        }
    };

    (*config).config.verify_mode = Some(VerifyMode::RootCert { root_certificate });

    0
}

/// Set TLS root certificate from raw PEM bytes.
///
/// This C API is preserved for backward compatibility with Android and iOS clients.
/// Internally converts to `VerifyMode::RootCert`.
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_tls_root_cert_bytes(
    config: *mut libclient_ConnectConfig,
    bytes: *const c_char,
    len: libc::size_t,
) -> c_int {
    let bytes = std::slice::from_raw_parts(bytes as *const u8, len);

    // Parse PEM and extract DER bytes
    let root_certificate = match rustls_pemfile::read_one_from_slice(bytes) {
        Ok(Some((rustls_pemfile::Item::X509Certificate(cert), _))) => cert.to_vec(),
        Ok(_) => {
            error!("Given bytes don't contain a X509 Certificate");
            return -1;
        }
        Err(e) => {
            error!("Failed to parse TLS certificate bytes: {:?}", e);
            return -1;
        }
    };

    (*config).config.verify_mode = Some(VerifyMode::RootCert { root_certificate });

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_streamer_port(
    config: *mut libclient_ConnectConfig,
    port: u16,
) {
    (*config).config.port = port;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_auto_reconnection(
    config: *mut libclient_ConnectConfig,
    enabled: bool,
) {
    (*config).config.auto_reconnection = enabled;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_unref(cfg: *mut libclient_ConnectConfig) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

/// Set TLS certificate fingerprint for pinning verification.
/// The fingerprint should be in format "sha256:xx:xx:xx:..."
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_set_tls_fingerprint(
    config: *mut libclient_ConnectConfig,
    fingerprint: *const c_char,
) -> c_int {
    let Ok(fingerprint) = CStr::from_ptr(fingerprint).to_str() else {
        warn!("Fail to convert TLS fingerprint");
        return -1;
    };

    result_to_c_int((*config).config.set_tls_fingerprint(fingerprint))
}

/// Skip TLS verification (DANGER - for testing only)
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_skip_tls_verification(
    config: *mut libclient_ConnectConfig,
) {
    (*config).config.skip_tls_verification();
}

/// TOFU prompt type
#[repr(C)]
#[allow(non_camel_case_types)]
pub enum libclient_TofuPromptType {
    UnknownHost,
    CertificateChanged,
}

/// TOFU prompt data passed to callback
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct libclient_TofuPrompt {
    pub prompt_type: libclient_TofuPromptType,
    pub hostname: *const c_char,
    pub fingerprint: *const c_char,          // For UnknownHost
    pub expected_fingerprint: *const c_char, // For CertificateChanged
    pub actual_fingerprint: *const c_char,   // For CertificateChanged
}

/// TOFU callback type - returns true to accept, false to reject
#[allow(non_camel_case_types)]
pub type libclient_TofuCallback = Option<
    unsafe extern "C" fn(prompt: *const libclient_TofuPrompt, userdata: *mut c_void) -> bool,
>;

/// Wrapper for raw pointer to make it Send + Sync.
/// SAFETY: The caller must ensure the pointer remains valid and thread-safe.
#[derive(Clone, Copy)]
struct SendSyncPtr(*mut c_void);
unsafe impl Send for SendSyncPtr {}
unsafe impl Sync for SendSyncPtr {}

impl SendSyncPtr {
    fn as_ptr(self) -> *mut c_void {
        self.0
    }
}

/// Enable TOFU (Trust-On-First-Use) verification with callback
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_enable_tofu(
    config: *mut libclient_ConnectConfig,
    callback: libclient_TofuCallback,
    userdata: *mut c_void,
) -> c_int {
    use crate::platform::tls::TofuPrompt;

    let Some(callback) = callback else {
        warn!("TOFU callback is null");
        return -1;
    };

    // Wrap the C callback in a Rust closure
    // SAFETY: userdata must remain valid for the lifetime of the connection
    let userdata = SendSyncPtr(userdata);
    let prompt_callback = move |prompt: TofuPrompt| -> bool {
        let userdata = userdata.as_ptr();

        let (prompt_type, hostname_cstr, fingerprint_cstr, expected_cstr, actual_cstr) =
            match &prompt {
                TofuPrompt::UnknownHost {
                    hostname,
                    fingerprint,
                } => {
                    let hostname_cstr = CString::new(hostname.as_str()).unwrap_or_default();
                    let fingerprint_cstr = CString::new(fingerprint.as_str()).unwrap_or_default();
                    (
                        libclient_TofuPromptType::UnknownHost,
                        hostname_cstr,
                        fingerprint_cstr,
                        CString::default(),
                        CString::default(),
                    )
                }
                TofuPrompt::CertificateChanged {
                    hostname,
                    expected_fingerprint,
                    actual_fingerprint,
                } => {
                    let hostname_cstr = CString::new(hostname.as_str()).unwrap_or_default();
                    let expected_cstr =
                        CString::new(expected_fingerprint.as_str()).unwrap_or_default();
                    let actual_cstr = CString::new(actual_fingerprint.as_str()).unwrap_or_default();
                    (
                        libclient_TofuPromptType::CertificateChanged,
                        hostname_cstr,
                        CString::default(),
                        expected_cstr,
                        actual_cstr,
                    )
                }
            };

        let c_prompt = libclient_TofuPrompt {
            prompt_type,
            hostname: hostname_cstr.as_ptr(),
            fingerprint: fingerprint_cstr.as_ptr(),
            expected_fingerprint: expected_cstr.as_ptr(),
            actual_fingerprint: actual_cstr.as_ptr(),
        };

        unsafe { callback(&c_prompt, userdata) }
    };

    result_to_c_int((*config).config.enable_tofu(prompt_callback))
}

/// Enable TOFU with auto-accept for new hosts
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_enable_tofu_auto_accept(
    config: *mut libclient_ConnectConfig,
) -> c_int {
    result_to_c_int((*config).config.enable_tofu_auto_accept())
}

/// Enable strict known_hosts mode (reject unknown hosts)
#[no_mangle]
pub unsafe extern "C" fn libclient_connect_config_enable_known_hosts_strict(
    config: *mut libclient_ConnectConfig,
) -> c_int {
    result_to_c_int((*config).config.enable_known_hosts_strict())
}

/// Add a host to known_hosts file
#[no_mangle]
pub unsafe extern "C" fn libclient_known_hosts_add(
    hostname: *const c_char,
    fingerprint: *const c_char,
) -> c_int {
    use crate::platform::known_hosts;

    let Ok(hostname) = CStr::from_ptr(hostname).to_str() else {
        warn!("Fail to convert hostname");
        return -1;
    };

    let Ok(fingerprint) = CStr::from_ptr(fingerprint).to_str() else {
        warn!("Fail to convert fingerprint");
        return -1;
    };

    result_to_c_int(known_hosts::add_host(hostname, fingerprint))
}

/// Remove a host from known_hosts file
/// Returns 0 if removed, 1 if not found, -1 on error
#[no_mangle]
pub unsafe extern "C" fn libclient_known_hosts_remove(hostname: *const c_char) -> c_int {
    use crate::platform::known_hosts;

    let Ok(hostname) = CStr::from_ptr(hostname).to_str() else {
        warn!("Fail to convert hostname");
        return -1;
    };

    match known_hosts::remove_host(hostname) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(_) => -1,
    }
}

/// Known hosts entry for listing
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct libclient_KnownHostEntry {
    pub hostname: *mut c_char,
    pub fingerprint: *mut c_char,
}

/// List all hosts in known_hosts file
/// Returns array of entries and count, caller must free with libclient_known_hosts_list_free
#[no_mangle]
pub unsafe extern "C" fn libclient_known_hosts_list(
    out_entries: *mut *mut libclient_KnownHostEntry,
    out_count: *mut usize,
) -> c_int {
    use crate::platform::known_hosts;

    let hosts = match known_hosts::list_hosts() {
        Ok(h) => h,
        Err(_) => return -1,
    };

    let count = hosts.len();
    if count == 0 {
        *out_entries = std::ptr::null_mut();
        *out_count = 0;
        return 0;
    }

    let entries = libc::malloc(count * std::mem::size_of::<libclient_KnownHostEntry>())
        as *mut libclient_KnownHostEntry;
    if entries.is_null() {
        return -1;
    }

    for (i, (hostname, fingerprint)) in hosts.into_iter().enumerate() {
        let hostname_cstr = CString::new(hostname).unwrap_or_default();
        let fingerprint_cstr = CString::new(fingerprint).unwrap_or_default();

        (*entries.add(i)).hostname = hostname_cstr.into_raw();
        (*entries.add(i)).fingerprint = fingerprint_cstr.into_raw();
    }

    *out_entries = entries;
    *out_count = count;
    0
}

/// Free known hosts list returned by libclient_known_hosts_list
#[no_mangle]
pub unsafe extern "C" fn libclient_known_hosts_list_free(
    entries: *mut libclient_KnownHostEntry,
    count: usize,
) {
    if entries.is_null() {
        return;
    }

    for i in 0..count {
        let entry = &mut *entries.add(i);
        if !entry.hostname.is_null() {
            let _ = CString::from_raw(entry.hostname);
        }
        if !entry.fingerprint.is_null() {
            let _ = CString::from_raw(entry.fingerprint);
        }
    }

    libc::free(entries as *mut c_void);
}

#[cfg(feature = "rtp")]
#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_protocol_rtp() -> *mut libclient_StreamingProtocol {
    let protocol = crate::StreamingProtocol::Rtp;

    Box::into_raw(Box::new(libclient_StreamingProtocol { protocol }))
}

#[cfg(feature = "kymux")]
#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_protocol_kymux(
    video_protocol: libclient_KymuxVideoProtocol,
    audio_protocol: libclient_KymuxAudioProtocol,
) -> *mut libclient_StreamingProtocol {
    let protocol = crate::StreamingProtocol::Kymux(crate::kymux_backend::KymuxConfig {
        video_protocol: Some(video_protocol.into()),
        audio_protocol: Some(audio_protocol.into()),
        backend: crate::kymux_backend::KymuxTransportBackend::Quic,
    });

    Box::into_raw(Box::new(libclient_StreamingProtocol { protocol }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_protocol_unref(
    protocol: *mut libclient_StreamingProtocol,
) {
    if !protocol.is_null() {
        let _ = Box::from_raw(protocol);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_player_config_unref(
    cfg: *mut libclient_VideoPlayerConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_player_config_default(
    window_handle: *mut libclient_WindowHandle,
    out_config: *mut *mut libclient_VideoPlayerConfig,
) -> c_int {
    if window_handle.is_null() {
        return -1;
    }

    let window_handle = Box::from_raw(window_handle);

    let config = crate::VideoPlayerConfig {
        window_handle: window_handle.handle,
        software_decode: false,
        video_buffer: 0,
    };

    *out_config = Box::into_raw(Box::new(libclient_VideoPlayerConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_default(
    display_id: u32,
    player: *mut libclient_VideoPlayerConfig,
    out_config: *mut *mut libclient_VideoSessionConfig,
) -> c_int {
    let config = crate::VideoSessionConfig {
        display_id,
        codec: VideoCodec::H264,
        yuv444: false,
        intra_refresh: None,
        player: (*player).config.clone(),
    };

    *out_config = Box::into_raw(Box::new(libclient_VideoSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_player_config_set_software_decode(
    config: *mut libclient_VideoPlayerConfig,
    software_decode: bool,
) {
    (*config).config.software_decode = software_decode;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_player_config_set_video_buffer(
    config: *mut libclient_VideoPlayerConfig,
    video_buffer: u32,
) {
    (*config).config.video_buffer = video_buffer;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_unref(
    cfg: *mut libclient_VideoSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_set_yuv444(
    config: *mut libclient_VideoSessionConfig,
    yuv444: bool,
) {
    (*config).config.yuv444 = yuv444;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_set_codec(
    config: *mut libclient_VideoSessionConfig,
    codec: VideoCodec,
) {
    (*config).config.codec = codec;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_video_session_config_set_intra_refresh(
    config: *mut libclient_VideoSessionConfig,
    intra_refresh: bool,
) {
    (*config).config.intra_refresh = Some(intra_refresh);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_audio_session_config_default(
    out_config: *mut *mut libclient_AudioSessionConfig,
) -> c_int {
    let config = crate::AudioSessionConfig { audio_buffer: None };
    *out_config = Box::into_raw(Box::new(libclient_AudioSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_audio_session_config_set_audio_buffer(
    config: *mut libclient_AudioSessionConfig,
    audio_buffer: u32,
) {
    (*config).config.audio_buffer = if audio_buffer != 0 {
        Some(audio_buffer)
    } else {
        None
    };
}

#[no_mangle]
pub unsafe extern "C" fn libclient_input_session_config_default(
    pipeline: *mut kynput_sys::kynput_Pipeline,
    out_config: *mut *mut libclient_InputSessionConfig,
) -> c_int {
    let pipeline = Pipeline::from_ptr(pipeline);

    let config = crate::InputSessionConfig {
        pipeline,
        gamepad_force_xbox360: None,
        clipboard: true,
    };

    *out_config = Box::into_raw(Box::new(libclient_InputSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_input_session_config_unref(
    cfg: *mut libclient_InputSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_input_session_config_gamepad_force_xbox360(
    cfg: *mut libclient_InputSessionConfig,
    gamepad_force_xbox360: bool,
) {
    (*cfg).config.gamepad_force_xbox360 = Some(gamepad_force_xbox360);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_input_session_config_clipboard(
    cfg: *mut libclient_InputSessionConfig,
    enable: bool,
) {
    (*cfg).config.clipboard = enable;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_default(
    protocol: *const libclient_StreamingProtocol,
    out_config: *mut *mut libclient_StreamingSessionConfig,
) -> c_int {
    let config = crate::StreamingSessionConfig {
        protocol: (*protocol).protocol.clone(),
        bitrate: 20_000_000,
        video: vec![],
        audio: None,
        input: None,
    };

    *out_config = Box::into_raw(Box::new(libclient_StreamingSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_unref(
    cfg: *mut libclient_StreamingSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_bitrate(
    config: *mut libclient_StreamingSessionConfig,
    bitrate: u32,
) {
    (*config).config.bitrate = bitrate;
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_get_bitrate(
    config: *mut libclient_StreamingSessionConfig,
) -> u32 {
    (*config).config.bitrate
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_audio(
    config: *mut libclient_StreamingSessionConfig,
    audio: *mut libclient_AudioSessionConfig,
) {
    let audio = Box::from_raw(audio);

    (*config).config.audio = Some(audio.config);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_add_video(
    config: *mut libclient_StreamingSessionConfig,
    video: *mut libclient_VideoSessionConfig,
) {
    let video = Box::from_raw(video);

    (*config).config.video.push(video.config);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_streaming_session_config_set_input(
    config: *mut libclient_StreamingSessionConfig,
    input: *mut libclient_InputSessionConfig,
) {
    let input = Box::from_raw(input);

    (*config).config.input = Some(input.config);
}

#[allow(non_camel_case_types)]
pub struct libclient_Client {
    client: crate::Client,
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_new(
    connect_config: *mut libclient_ConnectConfig,
    metrics: bool,
) -> *mut libclient_Client {
    let connect_config = (*connect_config).config.clone();

    let client = crate::Client::new(connect_config, metrics);
    if let Ok(client) = client {
        Box::into_raw(Box::new(libclient_Client { client }))
    } else {
        ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_stop(client: *mut libclient_Client) -> c_int {
    let ret = (&(*client)).client.stop();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_disconnect_and_stop(
    client: *mut libclient_Client,
    stop_session: bool,
) -> c_int {
    let ret = (&(*client)).client.disconnect_and_stop(stop_session);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_connect(client: *mut libclient_Client) -> c_int {
    let ret = (&(*client)).client.connect();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_disconnect(client: *mut libclient_Client) -> c_int {
    let ret = (&(*client)).client.disconnect();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_start_streaming(
    client: *mut libclient_Client,
    session_config: *mut libclient_StreamingSessionConfig,
) -> c_int {
    let session_config = (*session_config).config.clone();

    let ret = (&(*client)).client.start_streaming(session_config);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_stop_streaming(client: *mut libclient_Client) -> c_int {
    let ret = (&(*client)).client.stop_streaming();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_audio_set_volume(
    client: *mut libclient_Client,
    volume: f32,
) -> c_int {
    let ret = (&(*client)).client.audio_set_volume(volume);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_audio_set_mute(
    client: *mut libclient_Client,
    mute: bool,
) -> c_int {
    let ret = (&(*client)).client.audio_set_mute(mute);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_set_bitrate(
    client: *mut libclient_Client,
    bitrate: u32,
) -> c_int {
    let ret = (&(*client)).client.set_bitrate(bitrate);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn libclient_client_unref(client: *mut libclient_Client) {
    let _ = Box::from_raw(client);
}

#[allow(non_camel_case_types)]
pub type Event = super::Event;

#[repr(C)]
pub enum libclient_EventType {
    ControlPlaneConnected,
    ControlPlaneConnectFailed,
    DataPlaneConnectFailed,
    DataPlaneClosed,
    Reconnecting,
    Reconnected,
    ReconnectionFailed,
    Stopped,
    DisplayListUpdated,
    InputServiceConnected,
    InputServiceDisconnected,
    StreamingStarted,
    StreamingStartFailed,
    StreamingStopped,
    StreamerPipelineEvent,
    AudioVolumeChanged,
    AudioMuteChanged,
    BitrateChanged,
}

#[repr(C)]
pub enum libclient_StopReason {
    Requested,
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[repr(C)]
pub enum libclient_StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[repr(C)]
pub enum libclient_StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[repr(C)]
pub enum libclient_StreamerPipelineEventType {
    Stopped,
    Error,
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_get_type(event: *const Event) -> libclient_EventType {
    match *event {
        Event::ControlPlaneConnected { .. } => libclient_EventType::ControlPlaneConnected,
        Event::ControlPlaneConnectFailed => libclient_EventType::ControlPlaneConnectFailed,
        Event::DataPlaneConnectFailed => libclient_EventType::DataPlaneConnectFailed,
        Event::DataPlaneClosed => libclient_EventType::DataPlaneClosed,
        Event::Reconnecting { .. } => libclient_EventType::Reconnecting,
        Event::Reconnected { .. } => libclient_EventType::Reconnected,
        Event::ReconnectionFailed => libclient_EventType::ReconnectionFailed,
        Event::Stopped { .. } => libclient_EventType::Stopped,
        Event::DisplayListUpdated { .. } => libclient_EventType::DisplayListUpdated,
        Event::InputServiceConnected => libclient_EventType::InputServiceConnected,
        Event::InputServiceDisconnected => libclient_EventType::InputServiceDisconnected,
        Event::StreamingStarted => libclient_EventType::StreamingStarted,
        Event::StreamingStartFailed => libclient_EventType::StreamingStartFailed,
        Event::StreamingStopped => libclient_EventType::StreamingStopped,
        Event::StreamerPipelineEvent { .. } => libclient_EventType::StreamerPipelineEvent,
        Event::AudioVolumeChanged { .. } => libclient_EventType::AudioVolumeChanged,
        Event::AudioMuteChanged { .. } => libclient_EventType::AudioMuteChanged,
        Event::BitrateChanged { .. } => libclient_EventType::BitrateChanged,
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_control_plane_connected(
    event: *const Event,
    out_displays: *mut *mut Display,
    out_len: *mut u32,
) -> c_int {
    if let Event::ControlPlaneConnected { displays } = &*event {
        let mut displays = Box::new(displays.clone());

        *out_displays = displays.as_mut_ptr();
        *out_len = displays.len() as u32;

        std::mem::forget(displays);

        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_stopped(
    event: *const Event,
    out_reason: *mut libclient_StopReason,
) -> c_int {
    let Event::Stopped { reason } = &*event else {
        return -1;
    };

    *out_reason = match reason {
        crate::StopReason::Requested => libclient_StopReason::Requested,
        crate::StopReason::ControlPlaneClosed => libclient_StopReason::ControlPlaneClosed,
        crate::StopReason::ControlPlaneIoError => libclient_StopReason::ControlPlaneIoError,
        crate::StopReason::ControlPlaneTimeout => libclient_StopReason::ControlPlaneTimeout,
        crate::StopReason::DecodeError => libclient_StopReason::DecodeError,
    };

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_reconnecting(
    event: *const Event,
    out_attempt: *mut u32,
    out_max_attempts: *mut u32,
) -> c_int {
    let Event::Reconnecting {
        attempt,
        max_attempts,
    } = &*event
    else {
        return -1;
    };

    *out_attempt = *attempt;
    *out_max_attempts = *max_attempts;

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_reconnected(
    event: *const Event,
    out_should_restart_streaming: *mut bool,
) -> c_int {
    let Event::Reconnected {
        should_restart_streaming,
    } = &*event
    else {
        return -1;
    };

    *out_should_restart_streaming = *should_restart_streaming;

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_display_list_updated(
    event: *const Event,
    out_displays: *mut *mut Display,
    out_len: *mut u32,
) -> c_int {
    if let Event::DisplayListUpdated { displays } = &*event {
        let mut displays = Box::new(displays.clone());

        *out_displays = displays.as_mut_ptr();
        *out_len = displays.len() as u32;

        std::mem::forget(displays);

        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_free_displays(displays: *mut Display) {
    let _ = Box::from_raw(displays);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_streamer_pipeline_event(
    event: *const Event,
    out_type: *mut libclient_StreamerPipelineType,
    out_stage: *mut libclient_StreamerPipelineStage,
    out_event: *mut libclient_StreamerPipelineEventType,
) -> c_int {
    let Event::StreamerPipelineEvent {
        type_,
        stage,
        event,
    } = &*event
    else {
        return -1;
    };

    if out_type.is_null() || out_stage.is_null() || out_event.is_null() {
        return -1;
    }

    *out_type = match type_ {
        crate::StreamerPipelineType::EnumerateDisplays => {
            libclient_StreamerPipelineType::EnumerateDisplays
        }
        crate::StreamerPipelineType::Audio => libclient_StreamerPipelineType::Audio,
        crate::StreamerPipelineType::Video => libclient_StreamerPipelineType::Video,
    };

    *out_stage = match stage {
        crate::StreamerPipelineStage::Process => libclient_StreamerPipelineStage::Process,
        crate::StreamerPipelineStage::Initialization => {
            libclient_StreamerPipelineStage::Initialization
        }
        crate::StreamerPipelineStage::Acquisition => libclient_StreamerPipelineStage::Acquisition,
        crate::StreamerPipelineStage::Encoder => libclient_StreamerPipelineStage::Encoder,
    };

    *out_event = match event {
        crate::StreamerPipelineEventType::Stopped => libclient_StreamerPipelineEventType::Stopped,
        crate::StreamerPipelineEventType::Error => libclient_StreamerPipelineEventType::Error,
    };

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_audio_volume_changed(
    event: *const Event,
    out_volume: *mut f32,
) -> c_int {
    let Event::AudioVolumeChanged { volume } = &*event else {
        return -1;
    };

    *out_volume = *volume;

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_audio_mute_changed(
    event: *const Event,
    out_mute: *mut bool,
) -> c_int {
    let Event::AudioMuteChanged { mute } = &*event else {
        return -1;
    };

    *out_mute = *mute;

    0
}

#[no_mangle]
pub unsafe extern "C" fn libclient_event_unpack_bitrate_changed(
    event: *const Event,
    out_bitrate: *mut u32,
) -> c_int {
    let Event::BitrateChanged { bitrate } = &*event else {
        return -1;
    };

    *out_bitrate = *bitrate;

    0
}

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
    (&(*client))
        .client
        .event_manager
        .add_event_sink(Box::new(handler));

    0
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub enum libclient_MetricType {
    Video,
    NetworkLocal,
    NetworkPing,
    NetworkRemote,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct libclient_VideoMetric {
    pub source_id: u32,
    pub pts: u64,
    pub ts: i64,
    pub data: libclient_VideoMetricData,
}

impl From<VideoMetric> for libclient_VideoMetric {
    fn from(video_metric: VideoMetric) -> Self {
        Self {
            source_id: video_metric.source_id,
            pts: video_metric.pts,
            ts: video_metric.ts,
            data: video_metric.data.into(),
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub enum libclient_VideoMetricData {
    FrameAcquired,
    FrameEncoding,
    FrameEncoded,
    FrameSent,
    FrameReceived,
    FrameDecoding,
    FrameDecoded,
    FramePrepared,
    FrameDisplayed,
    FrameSkipped,
}

impl From<VideoMetricData> for libclient_VideoMetricData {
    fn from(video_metric_data: VideoMetricData) -> Self {
        match video_metric_data {
            VideoMetricData::FrameAcquired => libclient_VideoMetricData::FrameAcquired,
            VideoMetricData::FrameEncoding => libclient_VideoMetricData::FrameEncoding,
            VideoMetricData::FrameEncoded => libclient_VideoMetricData::FrameEncoded,
            VideoMetricData::FrameSent => libclient_VideoMetricData::FrameSent,
            VideoMetricData::FrameReceived => libclient_VideoMetricData::FrameReceived,
            VideoMetricData::FrameDecoding => libclient_VideoMetricData::FrameDecoding,
            VideoMetricData::FrameDecoded => libclient_VideoMetricData::FrameDecoded,
            VideoMetricData::FramePrepared => libclient_VideoMetricData::FramePrepared,
            VideoMetricData::FrameDisplayed => libclient_VideoMetricData::FrameDisplayed,
            VideoMetricData::FrameSkipped => libclient_VideoMetricData::FrameSkipped,
        }
    }
}

impl From<libclient_VideoMetricData> for VideoMetricData {
    fn from(data: libclient_VideoMetricData) -> Self {
        match data {
            libclient_VideoMetricData::FrameAcquired => VideoMetricData::FrameAcquired,
            libclient_VideoMetricData::FrameEncoding => VideoMetricData::FrameEncoding,
            libclient_VideoMetricData::FrameEncoded => VideoMetricData::FrameEncoded,
            libclient_VideoMetricData::FrameSent => VideoMetricData::FrameSent,
            libclient_VideoMetricData::FrameReceived => VideoMetricData::FrameReceived,
            libclient_VideoMetricData::FrameDecoding => VideoMetricData::FrameDecoding,
            libclient_VideoMetricData::FrameDecoded => VideoMetricData::FrameDecoded,
            libclient_VideoMetricData::FramePrepared => VideoMetricData::FramePrepared,
            libclient_VideoMetricData::FrameDisplayed => VideoMetricData::FrameDisplayed,
            libclient_VideoMetricData::FrameSkipped => VideoMetricData::FrameSkipped,
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct libclient_NetworkStats {
    pub rtt_micros: i64,
    pub packets_lost: i64,
    pub dropped_packets: i64,
}

impl From<NetworkStats> for libclient_NetworkStats {
    fn from(network_stats: NetworkStats) -> Self {
        Self {
            rtt_micros: network_stats.rtt_micros.map(|v| v as i64).unwrap_or(-1),
            packets_lost: network_stats.packets_lost.map(|v| v as i64).unwrap_or(-1),
            dropped_packets: network_stats
                .dropped_packets
                .map(|v| v as i64)
                .unwrap_or(-1),
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct libclient_NetworkPing {
    pub offset_micros: i64,
    pub delay_micros: i64,
}

impl From<NetworkPing> for libclient_NetworkPing {
    fn from(network_ping: NetworkPing) -> Self {
        Self {
            offset_micros: network_ping.offset_micros,
            delay_micros: network_ping.delay_micros,
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub union libclient_MetricData {
    pub video: libclient_VideoMetric,
    pub network_local: libclient_NetworkStats,
    pub network_ping: libclient_NetworkPing,
    pub network_remote: libclient_NetworkStats,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct libclient_Metric {
    pub type_: libclient_MetricType,
    pub data: libclient_MetricData,
}

impl From<Metric> for libclient_Metric {
    fn from(metric: Metric) -> Self {
        match metric {
            Metric::Video(video_metric) => Self {
                type_: libclient_MetricType::Video,
                data: libclient_MetricData {
                    video: video_metric.into(),
                },
            },
            Metric::NetworkLocal(network_stats) => Self {
                type_: libclient_MetricType::NetworkLocal,
                data: libclient_MetricData {
                    network_local: network_stats.into(),
                },
            },
            Metric::NetworkPing(network_ping) => Self {
                type_: libclient_MetricType::NetworkPing,
                data: libclient_MetricData {
                    network_ping: network_ping.into(),
                },
            },
            Metric::NetworkRemote(network_stats) => Self {
                type_: libclient_MetricType::NetworkRemote,
                data: libclient_MetricData {
                    network_remote: network_stats.into(),
                },
            },
        }
    }
}

#[allow(non_camel_case_types)]
pub type libclient_metrics_cb =
    Option<extern "C" fn(metric: *const libclient_Metric, userdata: *mut c_void)>;

#[no_mangle]
pub unsafe extern "C" fn libclient_client_set_metrics_callback(
    client: *mut libclient_Client,
    metrics_cb: libclient_metrics_cb,
    userdata: *mut c_void,
) {
    let cb: Option<Box<MetricsCallback>> = if let Some(metrics_cb) = metrics_cb {
        let metrics_cb = Arc::new(metrics_cb);
        let userdata = Arc::new(AtomicPtr::new(userdata));
        let cb = move |metric: Metric| unsafe {
            let userdata = *userdata.as_ptr();
            let libclient_metric: libclient_Metric = metric.into();
            metrics_cb(std::ptr::from_ref(&libclient_metric), userdata);
        };

        Some(Box::new(cb))
    } else {
        None
    };

    let client = unsafe { &(*client).client };
    client.set_metrics_callback(cb);
}

#[allow(non_camel_case_types)]
pub struct libclient_metrics_json_producer {
    producer: MetricsJSONProducer,
}

#[allow(non_camel_case_types)]
pub struct libclient_metrics_json {
    _data: Vec<CString>,
    c_data: Vec<*const c_char>,
}

#[no_mangle]
pub unsafe extern "C" fn libclient_metrics_json_producer_new(
) -> *mut libclient_metrics_json_producer {
    let producer = MetricsJSONProducer::new();
    Box::into_raw(Box::new(libclient_metrics_json_producer { producer }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_metrics_json_producer_unref(
    producer: *mut libclient_metrics_json_producer,
) {
    let _ = Box::from_raw(producer);
}

#[no_mangle]
pub unsafe extern "C" fn libclient_metrics_json_producer_push(
    producer: *mut libclient_metrics_json_producer,
    metric: *const libclient_Metric,
) -> *mut libclient_metrics_json {
    let producer = &mut (*producer).producer;
    let metric = &(*metric);

    let map_value = |v: i64| -> Option<u64> {
        if v >= 0 {
            Some(v as u64)
        } else {
            None
        }
    };

    let metric = match metric.type_ {
        libclient_MetricType::Video => {
            let data = &metric.data.video;

            Metric::Video(VideoMetric {
                source_id: data.source_id,
                pts: data.pts,
                ts: data.ts,
                data: data.data.into(),
            })
        }
        libclient_MetricType::NetworkLocal => {
            let data = &metric.data.network_local;

            Metric::NetworkLocal(NetworkStats {
                rtt_micros: map_value(data.rtt_micros),
                packets_lost: map_value(data.packets_lost),
                dropped_packets: map_value(data.dropped_packets),
            })
        }
        libclient_MetricType::NetworkPing => {
            let data = &metric.data.network_ping;

            Metric::NetworkPing(NetworkPing {
                offset_micros: data.offset_micros,
                delay_micros: data.delay_micros,
            })
        }
        libclient_MetricType::NetworkRemote => {
            let data = &metric.data.network_remote;

            Metric::NetworkRemote(NetworkStats {
                rtt_micros: map_value(data.rtt_micros),
                packets_lost: map_value(data.packets_lost),
                dropped_packets: map_value(data.dropped_packets),
            })
        }
    };

    let json = producer.push(&metric);

    let mut data = Vec::with_capacity(json.len());

    for l in json {
        let s = CString::from_vec_unchecked(l);
        data.push(s);
    }

    let c_data = data.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();

    Box::into_raw(Box::new(libclient_metrics_json {
        _data: data,
        c_data,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn libclient_metrics_json_values(
    json: *mut libclient_metrics_json,
    out_values: *mut *const *const c_char,
    out_count: *mut libc::size_t,
) {
    let json = &*json;

    *out_values = json.c_data.as_ptr();
    *out_count = json.c_data.len();
}

#[no_mangle]
pub unsafe extern "C" fn libclient_metrics_json_unref(json: *mut libclient_metrics_json) {
    let _ = Box::from_raw(json);
}
