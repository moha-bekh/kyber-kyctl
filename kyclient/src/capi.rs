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
pub struct kyclient_WindowHandle {
    handle: crate::platform::WindowHandle,
}

#[allow(non_camel_case_types)]
pub struct kyclient_ConnectConfig {
    config: crate::ConnectConfig,
}

#[allow(non_camel_case_types)]
pub struct kyclient_AuthCredentials {
    inner: crate::AuthCredentials,
}

#[allow(non_camel_case_types)]
pub struct kyclient_StreamingProtocol {
    protocol: crate::StreamingProtocol,
}

#[allow(non_camel_case_types)]
pub struct kyclient_StreamingSessionConfig {
    config: crate::StreamingSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct kyclient_VideoPlayerConfig {
    config: crate::VideoPlayerConfig,
}

#[allow(non_camel_case_types)]
pub struct kyclient_VideoSessionConfig {
    config: crate::VideoSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct kyclient_AudioSessionConfig {
    config: crate::AudioSessionConfig,
}

#[allow(non_camel_case_types)]
pub struct kyclient_InputSessionConfig {
    config: crate::InputSessionConfig,
}

fn result_to_c_int<T>(result: crate::Result<T>) -> c_int {
    match result {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[no_mangle]
pub extern "C" fn kyclient_init_crypto() {
    crate::init_crypto();
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum kyclient_KymuxVideoProtocol {
    Reliable,
    GopStream,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<kyclient_KymuxVideoProtocol> for kyproto::VideoProtocol {
    fn from(value: kyclient_KymuxVideoProtocol) -> Self {
        match value {
            kyclient_KymuxVideoProtocol::Reliable => Self::Reliable,
            kyclient_KymuxVideoProtocol::GopStream => Self::GopStream,
            kyclient_KymuxVideoProtocol::Unreliable => Self::Unreliable,
            kyclient_KymuxVideoProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum kyclient_KymuxAudioProtocol {
    Reliable,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<kyclient_KymuxAudioProtocol> for kyproto::AudioProtocol {
    fn from(value: kyclient_KymuxAudioProtocol) -> Self {
        match value {
            kyclient_KymuxAudioProtocol::Reliable => Self::Reliable,
            kyclient_KymuxAudioProtocol::Unreliable => Self::Unreliable,
            kyclient_KymuxAudioProtocol::UnreliableFec => Self::UnreliableFec,
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
pub unsafe extern "C" fn kyclient_set_log_cb(cb: LogCb, userdata: *mut c_void) {
    let userdata = std::sync::atomic::AtomicPtr::new(userdata);
    let logger = Box::new(Logger { cb, userdata });

    if log::set_boxed_logger(logger).is_err() {
        return;
    }

    log::set_max_level(log::LevelFilter::Trace);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_init_platform_verifier(
    env: ptr::NonNull<c_void>,
    context: ptr::NonNull<c_void>,
) -> c_int {
    let mut env =
        jni::JNIEnv::from_raw(env.as_ptr() as *mut jni::sys::JNIEnv).expect("invalid JNIEnv");
    let context = jni::objects::JObject::from_raw(context.as_ptr() as jni::sys::jobject);
    match rustls_platform_verifier::android::init_with_env(&mut env, context) {
        Ok(()) => 0,
        Err(e) => {
            error!("Failed to init platform verifier: {:?}", e);
            -1
        }
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_set_android_logger() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("kyclient"),
    );
}

#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_windowshandle_set_android_context(
    awindow_handler: *mut c_void,
) -> *mut kyclient_WindowHandle {
    log::info!(
        "kyclient_windowshandle_set_android_context {:?}",
        awindow_handler
    );
    Box::into_raw(Box::new(kyclient_WindowHandle {
        handle: crate::platform::WindowHandle::Android(awindow_handler as u64),
    }))
}

#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_windowshandle_from_x11(
    handle: u64,
) -> *mut kyclient_WindowHandle {
    Box::into_raw(Box::new(kyclient_WindowHandle {
        handle: crate::platform::WindowHandle::Xlib(handle),
    }))
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_windowshandle_from_ns_view(
    handle: u64,
) -> *mut kyclient_WindowHandle {
    Box::into_raw(Box::new(kyclient_WindowHandle {
        handle: crate::platform::WindowHandle::AppKit(handle),
    }))
}

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
#[no_mangle]
pub unsafe extern "C" fn kyclient_windowshandle_from_ui_view(
    handle: u64,
) -> *mut kyclient_WindowHandle {
    Box::into_raw(Box::new(kyclient_WindowHandle {
        handle: crate::platform::WindowHandle::UiKit(handle),
    }))
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_windowshandle_from_hwnd(hwnd: u64) -> *mut kyclient_WindowHandle {
    Box::into_raw(Box::new(kyclient_WindowHandle {
        handle: crate::platform::WindowHandle::Win32(hwnd),
    }))
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_auth_credentials_new_basic(
    username: *const c_char,
    password: *const c_char,
) -> *mut kyclient_AuthCredentials {
    let Ok(username) = CStr::from_ptr(username).to_str() else {
        warn!("Fail to convert username");
        return ptr::null_mut();
    };
    let username = username.to_owned();

    let Ok(password) = CStr::from_ptr(password).to_str() else {
        warn!("Fail to convert password");
        return ptr::null_mut();
    };
    let password = password.to_owned();

    Box::into_raw(Box::new(kyclient_AuthCredentials {
        inner: crate::AuthCredentials::Basic { username, password },
    }))
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_auth_credentials_new_jwt(
    token: *const c_char,
) -> *mut kyclient_AuthCredentials {
    let Ok(token) = CStr::from_ptr(token).to_str() else {
        warn!("Fail to convert token");
        return ptr::null_mut();
    };
    let token = token.to_owned();

    Box::into_raw(Box::new(kyclient_AuthCredentials {
        inner: crate::AuthCredentials::Jwt { token },
    }))
}

#[no_mangle]
/// # Safety
/// `token` must be a valid non-null null-terminated C string.
pub unsafe extern "C" fn kyclient_auth_credentials_new_oidc(
    token: *const c_char,
) -> *mut kyclient_AuthCredentials {
    let Ok(token) = CStr::from_ptr(token).to_str() else {
        warn!("Failed to convert OIDC token");
        return ptr::null_mut();
    };
    let token = token.to_owned();

    Box::into_raw(Box::new(kyclient_AuthCredentials {
        inner: crate::AuthCredentials::Oidc { token },
    }))
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_default(
    host: *const c_char,
    credentials: ptr::NonNull<kyclient_AuthCredentials>,
    out_config: *mut *mut kyclient_ConnectConfig,
) -> c_int {
    let Ok(host) = CStr::from_ptr(host).to_str() else {
        warn!("Fail to convert client Host");
        return -1;
    };

    let credentials = Box::from_raw(credentials.as_ptr());

    let config = crate::ConnectConfig {
        host: host.into(),
        port: crate::STREAMER_PORT,
        credentials: credentials.inner,
        tls_host: None,
        verify_mode: None,
        auto_reconnection: false,
    };

    *out_config = Box::into_raw(Box::new(kyclient_ConnectConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_set_credentials(
    config: *mut kyclient_ConnectConfig,
    credentials: *mut kyclient_AuthCredentials,
) -> c_int {
    let credentials = Box::from_raw(credentials);
    (*config).config.credentials = credentials.inner;

    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_host(
    config: *mut kyclient_ConnectConfig,
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
#[deprecated = "use kyclient_connect_config_set_tls_root_cert_path"]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_root_cert(
    config: *mut kyclient_ConnectConfig,
    tls_root_cert: *const c_char,
) -> c_int {
    kyclient_connect_config_set_tls_root_cert_path(config, tls_root_cert)
}

/// Set TLS root certificate from a PEM file path.
///
/// This C API is preserved for backward compatibility with Android and iOS clients.
/// Internally converts to `VerifyMode::RootCert` - the old `tls_config` codepath
/// has been removed in favor of the unified `verify_mode` system.
///
/// The path is resolved relative to the executable directory if it's a relative path.
#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_root_cert_path(
    config: *mut kyclient_ConnectConfig,
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
pub unsafe extern "C" fn kyclient_connect_config_set_tls_root_cert_bytes(
    config: *mut kyclient_ConnectConfig,
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
pub unsafe extern "C" fn kyclient_connect_config_set_streamer_port(
    config: *mut kyclient_ConnectConfig,
    port: u16,
) {
    (*config).config.port = port;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_set_auto_reconnection(
    config: *mut kyclient_ConnectConfig,
    enabled: bool,
) {
    (*config).config.auto_reconnection = enabled;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_unref(cfg: *mut kyclient_ConnectConfig) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

/// Set TLS certificate fingerprint for pinning verification.
/// The fingerprint should be in format "sha256:xx:xx:xx:..."
#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_fingerprint(
    config: *mut kyclient_ConnectConfig,
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
pub unsafe extern "C" fn kyclient_connect_config_skip_tls_verification(
    config: *mut kyclient_ConnectConfig,
) {
    (*config).config.skip_tls_verification();
}

/// TOFU prompt type
#[repr(C)]
#[allow(non_camel_case_types)]
pub enum kyclient_TofuPromptType {
    UnknownHost,
    CertificateChanged,
}

/// TOFU prompt data passed to callback
#[repr(C)]
#[allow(non_camel_case_types)]
pub struct kyclient_TofuPrompt {
    pub prompt_type: kyclient_TofuPromptType,
    pub hostname: *const c_char,
    pub fingerprint: *const c_char,          // For UnknownHost
    pub expected_fingerprint: *const c_char, // For CertificateChanged
    pub actual_fingerprint: *const c_char,   // For CertificateChanged
}

/// TOFU callback type - returns true to accept, false to reject
#[allow(non_camel_case_types)]
pub type kyclient_TofuCallback =
    Option<unsafe extern "C" fn(prompt: *const kyclient_TofuPrompt, userdata: *mut c_void) -> bool>;

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
pub unsafe extern "C" fn kyclient_connect_config_enable_tofu(
    config: *mut kyclient_ConnectConfig,
    callback: kyclient_TofuCallback,
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
                        kyclient_TofuPromptType::UnknownHost,
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
                        kyclient_TofuPromptType::CertificateChanged,
                        hostname_cstr,
                        CString::default(),
                        expected_cstr,
                        actual_cstr,
                    )
                }
            };

        let c_prompt = kyclient_TofuPrompt {
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
pub unsafe extern "C" fn kyclient_connect_config_enable_tofu_auto_accept(
    config: *mut kyclient_ConnectConfig,
) -> c_int {
    result_to_c_int((*config).config.enable_tofu_auto_accept())
}

/// Enable strict known_hosts mode (reject unknown hosts)
#[no_mangle]
pub unsafe extern "C" fn kyclient_connect_config_enable_known_hosts_strict(
    config: *mut kyclient_ConnectConfig,
) -> c_int {
    result_to_c_int((*config).config.enable_known_hosts_strict())
}

/// Add a host to known_hosts file
#[no_mangle]
pub unsafe extern "C" fn kyclient_known_hosts_add(
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
pub unsafe extern "C" fn kyclient_known_hosts_remove(hostname: *const c_char) -> c_int {
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
pub struct kyclient_KnownHostEntry {
    pub hostname: *mut c_char,
    pub fingerprint: *mut c_char,
}

/// List all hosts in known_hosts file
/// Returns array of entries and count, caller must free with kyclient_known_hosts_list_free
#[no_mangle]
pub unsafe extern "C" fn kyclient_known_hosts_list(
    out_entries: *mut *mut kyclient_KnownHostEntry,
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

    let entries = libc::malloc(count * std::mem::size_of::<kyclient_KnownHostEntry>())
        as *mut kyclient_KnownHostEntry;
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

/// Free known hosts list returned by kyclient_known_hosts_list
#[no_mangle]
pub unsafe extern "C" fn kyclient_known_hosts_list_free(
    entries: *mut kyclient_KnownHostEntry,
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
pub unsafe extern "C" fn kyclient_streaming_protocol_rtp() -> *mut kyclient_StreamingProtocol {
    let protocol = crate::StreamingProtocol::Rtp;

    Box::into_raw(Box::new(kyclient_StreamingProtocol { protocol }))
}

#[cfg(feature = "kymux")]
#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_protocol_kymux(
    video_protocol: kyclient_KymuxVideoProtocol,
    audio_protocol: kyclient_KymuxAudioProtocol,
) -> *mut kyclient_StreamingProtocol {
    let protocol = crate::StreamingProtocol::Kymux(crate::kymux_backend::KymuxConfig {
        video_protocol: Some(video_protocol.into()),
        audio_protocol: Some(audio_protocol.into()),
        backend: crate::kymux_backend::KymuxTransportBackend::Quic,
    });

    Box::into_raw(Box::new(kyclient_StreamingProtocol { protocol }))
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_protocol_unref(
    protocol: *mut kyclient_StreamingProtocol,
) {
    if !protocol.is_null() {
        let _ = Box::from_raw(protocol);
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_player_config_unref(cfg: *mut kyclient_VideoPlayerConfig) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_player_config_default(
    window_handle: *mut kyclient_WindowHandle,
    out_config: *mut *mut kyclient_VideoPlayerConfig,
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

    *out_config = Box::into_raw(Box::new(kyclient_VideoPlayerConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_session_config_default(
    display_id: u32,
    player: *mut kyclient_VideoPlayerConfig,
    out_config: *mut *mut kyclient_VideoSessionConfig,
) -> c_int {
    let config = crate::VideoSessionConfig {
        display_id,
        codec: VideoCodec::H264,
        yuv444: false,
        intra_refresh: None,
        player: (*player).config.clone(),
    };

    *out_config = Box::into_raw(Box::new(kyclient_VideoSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_player_config_set_software_decode(
    config: *mut kyclient_VideoPlayerConfig,
    software_decode: bool,
) {
    (*config).config.software_decode = software_decode;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_player_config_set_video_buffer(
    config: *mut kyclient_VideoPlayerConfig,
    video_buffer: u32,
) {
    (*config).config.video_buffer = video_buffer;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_session_config_unref(
    cfg: *mut kyclient_VideoSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_session_config_set_yuv444(
    config: *mut kyclient_VideoSessionConfig,
    yuv444: bool,
) {
    (*config).config.yuv444 = yuv444;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_session_config_set_codec(
    config: *mut kyclient_VideoSessionConfig,
    codec: VideoCodec,
) {
    (*config).config.codec = codec;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_video_session_config_set_intra_refresh(
    config: *mut kyclient_VideoSessionConfig,
    intra_refresh: bool,
) {
    (*config).config.intra_refresh = Some(intra_refresh);
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_audio_session_config_default(
    out_config: *mut *mut kyclient_AudioSessionConfig,
) -> c_int {
    let config = crate::AudioSessionConfig { audio_buffer: None };
    *out_config = Box::into_raw(Box::new(kyclient_AudioSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_audio_session_config_set_audio_buffer(
    config: *mut kyclient_AudioSessionConfig,
    audio_buffer: u32,
) {
    (*config).config.audio_buffer = if audio_buffer != 0 {
        Some(audio_buffer)
    } else {
        None
    };
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_input_session_config_default(
    pipeline: *mut kynput_sys::kynput_Pipeline,
    out_config: *mut *mut kyclient_InputSessionConfig,
) -> c_int {
    let pipeline = Pipeline::from_ptr(pipeline);

    let config = crate::InputSessionConfig {
        pipeline,
        gamepad_force_xbox360: None,
        clipboard: true,
    };

    *out_config = Box::into_raw(Box::new(kyclient_InputSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_input_session_config_unref(
    cfg: *mut kyclient_InputSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_input_session_config_gamepad_force_xbox360(
    cfg: *mut kyclient_InputSessionConfig,
    gamepad_force_xbox360: bool,
) {
    (*cfg).config.gamepad_force_xbox360 = Some(gamepad_force_xbox360);
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_input_session_config_clipboard(
    cfg: *mut kyclient_InputSessionConfig,
    enable: bool,
) {
    (*cfg).config.clipboard = enable;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_default(
    protocol: *const kyclient_StreamingProtocol,
    out_config: *mut *mut kyclient_StreamingSessionConfig,
) -> c_int {
    let config = crate::StreamingSessionConfig {
        protocol: (*protocol).protocol.clone(),
        bitrate: 20_000_000,
        video: vec![],
        audio: None,
        input: None,
    };

    *out_config = Box::into_raw(Box::new(kyclient_StreamingSessionConfig { config }));
    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_unref(
    cfg: *mut kyclient_StreamingSessionConfig,
) {
    if !cfg.is_null() {
        let _ = Box::from_raw(cfg);
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_set_bitrate(
    config: *mut kyclient_StreamingSessionConfig,
    bitrate: u32,
) {
    (*config).config.bitrate = bitrate;
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_get_bitrate(
    config: *mut kyclient_StreamingSessionConfig,
) -> u32 {
    (*config).config.bitrate
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_set_audio(
    config: *mut kyclient_StreamingSessionConfig,
    audio: *mut kyclient_AudioSessionConfig,
) {
    let audio = Box::from_raw(audio);

    (*config).config.audio = Some(audio.config);
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_add_video(
    config: *mut kyclient_StreamingSessionConfig,
    video: *mut kyclient_VideoSessionConfig,
) {
    let video = Box::from_raw(video);

    (*config).config.video.push(video.config);
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_streaming_session_config_set_input(
    config: *mut kyclient_StreamingSessionConfig,
    input: *mut kyclient_InputSessionConfig,
) {
    let input = Box::from_raw(input);

    (*config).config.input = Some(input.config);
}

#[allow(non_camel_case_types)]
pub struct kyclient_Client {
    client: crate::Client,
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_new(
    connect_config: *mut kyclient_ConnectConfig,
    metrics: bool,
) -> *mut kyclient_Client {
    let connect_config = (*connect_config).config.clone();

    let client = crate::Client::new(connect_config, metrics);
    if let Ok(client) = client {
        Box::into_raw(Box::new(kyclient_Client { client }))
    } else {
        ptr::null_mut()
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_stop(client: *mut kyclient_Client) -> c_int {
    let ret = (&(*client)).client.stop();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_disconnect_and_stop(
    client: *mut kyclient_Client,
    stop_session: bool,
) -> c_int {
    let ret = (&(*client)).client.disconnect_and_stop(stop_session);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_connect(client: *mut kyclient_Client) -> c_int {
    let ret = (&(*client)).client.connect();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_disconnect(client: *mut kyclient_Client) -> c_int {
    let ret = (&(*client)).client.disconnect();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_start_streaming(
    client: *mut kyclient_Client,
    session_config: *mut kyclient_StreamingSessionConfig,
) -> c_int {
    let session_config = (*session_config).config.clone();

    let ret = (&(*client)).client.start_streaming(session_config);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_stop_streaming(client: *mut kyclient_Client) -> c_int {
    let ret = (&(*client)).client.stop_streaming();
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_audio_set_volume(
    client: *mut kyclient_Client,
    volume: f32,
) -> c_int {
    let ret = (&(*client)).client.audio_set_volume(volume);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_audio_set_mute(
    client: *mut kyclient_Client,
    mute: bool,
) -> c_int {
    let ret = (&(*client)).client.audio_set_mute(mute);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_set_bitrate(
    client: *mut kyclient_Client,
    bitrate: u32,
) -> c_int {
    let ret = (&(*client)).client.set_bitrate(bitrate);
    result_to_c_int(ret)
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_unref(client: *mut kyclient_Client) {
    let _ = Box::from_raw(client);
}

#[allow(non_camel_case_types)]
pub type Event = super::Event;

#[repr(C)]
pub enum kyclient_EventType {
    ControlPlaneConnected,
    ControlPlaneConnectFailed,
    AuthenticationRequired,
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
pub enum kyclient_ConnectFailedReason {
    NetworkError,
    Timeout,
    Unauthorized,
    ServerError,
    TlsVerification,
    TlsConfiguration,
    WebSocket,
    Protocol,
}

#[repr(C)]
pub enum kyclient_StopReason {
    Requested,
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[repr(C)]
pub enum kyclient_StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[repr(C)]
pub enum kyclient_StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[repr(C)]
pub enum kyclient_StreamerPipelineEventType {
    Stopped,
    Error,
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_event_get_type(event: *const Event) -> kyclient_EventType {
    match *event {
        Event::ControlPlaneConnected { .. } => kyclient_EventType::ControlPlaneConnected,
        Event::ControlPlaneConnectFailed { .. } => kyclient_EventType::ControlPlaneConnectFailed,
        Event::AuthenticationRequired { .. } => kyclient_EventType::AuthenticationRequired,
        Event::DataPlaneConnectFailed => kyclient_EventType::DataPlaneConnectFailed,
        Event::DataPlaneClosed => kyclient_EventType::DataPlaneClosed,
        Event::Reconnecting { .. } => kyclient_EventType::Reconnecting,
        Event::Reconnected { .. } => kyclient_EventType::Reconnected,
        Event::ReconnectionFailed => kyclient_EventType::ReconnectionFailed,
        Event::Stopped { .. } => kyclient_EventType::Stopped,
        Event::DisplayListUpdated { .. } => kyclient_EventType::DisplayListUpdated,
        Event::InputServiceConnected => kyclient_EventType::InputServiceConnected,
        Event::InputServiceDisconnected => kyclient_EventType::InputServiceDisconnected,
        Event::StreamingStarted => kyclient_EventType::StreamingStarted,
        Event::StreamingStartFailed => kyclient_EventType::StreamingStartFailed,
        Event::StreamingStopped => kyclient_EventType::StreamingStopped,
        Event::StreamerPipelineEvent { .. } => kyclient_EventType::StreamerPipelineEvent,
        Event::AudioVolumeChanged { .. } => kyclient_EventType::AudioVolumeChanged,
        Event::AudioMuteChanged { .. } => kyclient_EventType::AudioMuteChanged,
        Event::BitrateChanged { .. } => kyclient_EventType::BitrateChanged,
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_event_unpack_control_plane_connected(
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
pub unsafe extern "C" fn kyclient_event_unpack_connect_failed(
    event: *const Event,
    out_reason: *mut kyclient_ConnectFailedReason,
) -> c_int {
    let Event::ControlPlaneConnectFailed { reason } = &*event else {
        return -1;
    };

    *out_reason = match reason {
        crate::ConnectFailedReason::NetworkError => kyclient_ConnectFailedReason::NetworkError,
        crate::ConnectFailedReason::Timeout => kyclient_ConnectFailedReason::Timeout,
        crate::ConnectFailedReason::Unauthorized => kyclient_ConnectFailedReason::Unauthorized,
        crate::ConnectFailedReason::ServerError => kyclient_ConnectFailedReason::ServerError,
        crate::ConnectFailedReason::TlsVerification => {
            kyclient_ConnectFailedReason::TlsVerification
        }
        crate::ConnectFailedReason::TlsConfiguration => {
            kyclient_ConnectFailedReason::TlsConfiguration
        }
        crate::ConnectFailedReason::WebSocket => kyclient_ConnectFailedReason::WebSocket,
        crate::ConnectFailedReason::Protocol => kyclient_ConnectFailedReason::Protocol,
    };

    0
}

#[no_mangle]
/// # Safety
/// `event` must be a valid `ControlPlaneAuthenticationRequired` event. On success,
/// `out_issuer` and `out_client_id` receive freshly-allocated C strings that the
/// caller must free with `kyclient_event_free_authentication_required`.
pub unsafe extern "C" fn kyclient_event_unpack_authentication_required(
    event: *const Event,
    out_issuer: *mut *mut c_char,
    out_client_id: *mut *mut c_char,
) -> c_int {
    let Event::AuthenticationRequired { issuer, client_id } = &*event else {
        return -1;
    };

    let Ok(c_issuer) = CString::new(issuer.clone()) else {
        return -1;
    };
    let Ok(c_client_id) = CString::new(client_id.clone()) else {
        return -1;
    };

    *out_issuer = c_issuer.into_raw();
    *out_client_id = c_client_id.into_raw();

    0
}

#[no_mangle]
/// # Safety
/// `issuer` and `client_id` must be pointers returned by
/// `kyclient_event_unpack_authentication_required`, and must not be freed more than once.
pub unsafe extern "C" fn kyclient_event_free_authentication_required(
    issuer: *mut c_char,
    client_id: *mut c_char,
) {
    if !issuer.is_null() {
        let _ = CString::from_raw(issuer);
    }
    if !client_id.is_null() {
        let _ = CString::from_raw(client_id);
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_event_unpack_stopped(
    event: *const Event,
    out_reason: *mut kyclient_StopReason,
) -> c_int {
    let Event::Stopped { reason } = &*event else {
        return -1;
    };

    *out_reason = match reason {
        crate::StopReason::Requested => kyclient_StopReason::Requested,
        crate::StopReason::ControlPlaneClosed => kyclient_StopReason::ControlPlaneClosed,
        crate::StopReason::ControlPlaneIoError => kyclient_StopReason::ControlPlaneIoError,
        crate::StopReason::ControlPlaneTimeout => kyclient_StopReason::ControlPlaneTimeout,
        crate::StopReason::DecodeError => kyclient_StopReason::DecodeError,
    };

    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_event_unpack_reconnecting(
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
pub unsafe extern "C" fn kyclient_event_unpack_reconnected(
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
pub unsafe extern "C" fn kyclient_event_unpack_display_list_updated(
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
pub unsafe extern "C" fn kyclient_event_free_displays(displays: *mut Display) {
    let _ = Box::from_raw(displays);
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_event_unpack_streamer_pipeline_event(
    event: *const Event,
    out_type: *mut kyclient_StreamerPipelineType,
    out_stage: *mut kyclient_StreamerPipelineStage,
    out_event: *mut kyclient_StreamerPipelineEventType,
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
            kyclient_StreamerPipelineType::EnumerateDisplays
        }
        crate::StreamerPipelineType::Audio => kyclient_StreamerPipelineType::Audio,
        crate::StreamerPipelineType::Video => kyclient_StreamerPipelineType::Video,
    };

    *out_stage = match stage {
        crate::StreamerPipelineStage::Process => kyclient_StreamerPipelineStage::Process,
        crate::StreamerPipelineStage::Initialization => {
            kyclient_StreamerPipelineStage::Initialization
        }
        crate::StreamerPipelineStage::Acquisition => kyclient_StreamerPipelineStage::Acquisition,
        crate::StreamerPipelineStage::Encoder => kyclient_StreamerPipelineStage::Encoder,
    };

    *out_event = match event {
        crate::StreamerPipelineEventType::Stopped => kyclient_StreamerPipelineEventType::Stopped,
        crate::StreamerPipelineEventType::Error => kyclient_StreamerPipelineEventType::Error,
    };

    0
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_event_unpack_audio_volume_changed(
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
pub unsafe extern "C" fn kyclient_event_unpack_audio_mute_changed(
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
pub unsafe extern "C" fn kyclient_event_unpack_bitrate_changed(
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
pub struct kyclient_EventHandler {
    on_event: Option<extern "C" fn(event: *const Event, userdata: *mut c_void)>,
    userdata: *mut c_void,
}

unsafe impl Send for kyclient_EventHandler {}
unsafe impl Sync for kyclient_EventHandler {}

impl crate::event_manager::EventSink for kyclient_EventHandler {
    fn on_event(&self, event: &crate::Event) {
        if let Some(on_event) = self.on_event {
            on_event(event, self.userdata);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_set_event_handler(
    client: *mut kyclient_Client,
    handler: kyclient_EventHandler,
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
pub enum kyclient_MetricType {
    Video,
    NetworkLocal,
    NetworkPing,
    NetworkRemote,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct kyclient_VideoMetric {
    pub source_id: u32,
    pub pts: u64,
    pub ts: i64,
    pub data: kyclient_VideoMetricData,
}

impl From<VideoMetric> for kyclient_VideoMetric {
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
pub enum kyclient_VideoMetricData {
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

impl From<VideoMetricData> for kyclient_VideoMetricData {
    fn from(video_metric_data: VideoMetricData) -> Self {
        match video_metric_data {
            VideoMetricData::FrameAcquired => kyclient_VideoMetricData::FrameAcquired,
            VideoMetricData::FrameEncoding => kyclient_VideoMetricData::FrameEncoding,
            VideoMetricData::FrameEncoded => kyclient_VideoMetricData::FrameEncoded,
            VideoMetricData::FrameSent => kyclient_VideoMetricData::FrameSent,
            VideoMetricData::FrameReceived => kyclient_VideoMetricData::FrameReceived,
            VideoMetricData::FrameDecoding => kyclient_VideoMetricData::FrameDecoding,
            VideoMetricData::FrameDecoded => kyclient_VideoMetricData::FrameDecoded,
            VideoMetricData::FramePrepared => kyclient_VideoMetricData::FramePrepared,
            VideoMetricData::FrameDisplayed => kyclient_VideoMetricData::FrameDisplayed,
            VideoMetricData::FrameSkipped => kyclient_VideoMetricData::FrameSkipped,
        }
    }
}

impl From<kyclient_VideoMetricData> for VideoMetricData {
    fn from(data: kyclient_VideoMetricData) -> Self {
        match data {
            kyclient_VideoMetricData::FrameAcquired => VideoMetricData::FrameAcquired,
            kyclient_VideoMetricData::FrameEncoding => VideoMetricData::FrameEncoding,
            kyclient_VideoMetricData::FrameEncoded => VideoMetricData::FrameEncoded,
            kyclient_VideoMetricData::FrameSent => VideoMetricData::FrameSent,
            kyclient_VideoMetricData::FrameReceived => VideoMetricData::FrameReceived,
            kyclient_VideoMetricData::FrameDecoding => VideoMetricData::FrameDecoding,
            kyclient_VideoMetricData::FrameDecoded => VideoMetricData::FrameDecoded,
            kyclient_VideoMetricData::FramePrepared => VideoMetricData::FramePrepared,
            kyclient_VideoMetricData::FrameDisplayed => VideoMetricData::FrameDisplayed,
            kyclient_VideoMetricData::FrameSkipped => VideoMetricData::FrameSkipped,
        }
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct kyclient_NetworkStats {
    pub rtt_micros: i64,
    pub packets_lost: i64,
    pub dropped_packets: i64,
}

impl From<NetworkStats> for kyclient_NetworkStats {
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
pub struct kyclient_NetworkPing {
    pub offset_micros: i64,
    pub delay_micros: i64,
}

impl From<NetworkPing> for kyclient_NetworkPing {
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
pub union kyclient_MetricData {
    pub video: kyclient_VideoMetric,
    pub network_local: kyclient_NetworkStats,
    pub network_ping: kyclient_NetworkPing,
    pub network_remote: kyclient_NetworkStats,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
pub struct kyclient_Metric {
    pub type_: kyclient_MetricType,
    pub data: kyclient_MetricData,
}

impl From<Metric> for kyclient_Metric {
    fn from(metric: Metric) -> Self {
        match metric {
            Metric::Video(video_metric) => Self {
                type_: kyclient_MetricType::Video,
                data: kyclient_MetricData {
                    video: video_metric.into(),
                },
            },
            Metric::NetworkLocal(network_stats) => Self {
                type_: kyclient_MetricType::NetworkLocal,
                data: kyclient_MetricData {
                    network_local: network_stats.into(),
                },
            },
            Metric::NetworkPing(network_ping) => Self {
                type_: kyclient_MetricType::NetworkPing,
                data: kyclient_MetricData {
                    network_ping: network_ping.into(),
                },
            },
            Metric::NetworkRemote(network_stats) => Self {
                type_: kyclient_MetricType::NetworkRemote,
                data: kyclient_MetricData {
                    network_remote: network_stats.into(),
                },
            },
        }
    }
}

#[allow(non_camel_case_types)]
pub type kyclient_metrics_cb =
    Option<extern "C" fn(metric: *const kyclient_Metric, userdata: *mut c_void)>;

#[no_mangle]
pub unsafe extern "C" fn kyclient_client_set_metrics_callback(
    client: *mut kyclient_Client,
    metrics_cb: kyclient_metrics_cb,
    userdata: *mut c_void,
) {
    let cb: Option<Box<MetricsCallback>> = if let Some(metrics_cb) = metrics_cb {
        let metrics_cb = Arc::new(metrics_cb);
        let userdata = Arc::new(AtomicPtr::new(userdata));
        let cb = move |metric: Metric| unsafe {
            let userdata = *userdata.as_ptr();
            let kyclient_metric: kyclient_Metric = metric.into();
            metrics_cb(std::ptr::from_ref(&kyclient_metric), userdata);
        };

        Some(Box::new(cb))
    } else {
        None
    };

    let client = unsafe { &(*client).client };
    client.set_metrics_callback(cb);
}

#[allow(non_camel_case_types)]
pub struct kyclient_metrics_json_producer {
    producer: MetricsJSONProducer,
}

#[allow(non_camel_case_types)]
pub struct kyclient_metrics_json {
    _data: Vec<CString>,
    c_data: Vec<*const c_char>,
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_metrics_json_producer_new() -> *mut kyclient_metrics_json_producer
{
    let producer = MetricsJSONProducer::new();
    Box::into_raw(Box::new(kyclient_metrics_json_producer { producer }))
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_metrics_json_producer_unref(
    producer: *mut kyclient_metrics_json_producer,
) {
    let _ = Box::from_raw(producer);
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_metrics_json_producer_push(
    producer: *mut kyclient_metrics_json_producer,
    metric: *const kyclient_Metric,
) -> *mut kyclient_metrics_json {
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
        kyclient_MetricType::Video => {
            let data = &metric.data.video;

            Metric::Video(VideoMetric {
                source_id: data.source_id,
                pts: data.pts,
                ts: data.ts,
                data: data.data.into(),
            })
        }
        kyclient_MetricType::NetworkLocal => {
            let data = &metric.data.network_local;

            Metric::NetworkLocal(NetworkStats {
                rtt_micros: map_value(data.rtt_micros),
                packets_lost: map_value(data.packets_lost),
                dropped_packets: map_value(data.dropped_packets),
            })
        }
        kyclient_MetricType::NetworkPing => {
            let data = &metric.data.network_ping;

            Metric::NetworkPing(NetworkPing {
                offset_micros: data.offset_micros,
                delay_micros: data.delay_micros,
            })
        }
        kyclient_MetricType::NetworkRemote => {
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

    Box::into_raw(Box::new(kyclient_metrics_json {
        _data: data,
        c_data,
    }))
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_metrics_json_values(
    json: *mut kyclient_metrics_json,
    out_values: *mut *const *const c_char,
    out_count: *mut libc::size_t,
) {
    let json = &*json;

    *out_values = json.c_data.as_ptr();
    *out_count = json.c_data.len();
}

#[no_mangle]
pub unsafe extern "C" fn kyclient_metrics_json_unref(json: *mut kyclient_metrics_json) {
    let _ = Box::from_raw(json);
}
