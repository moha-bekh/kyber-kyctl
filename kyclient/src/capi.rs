// Project Kyber: capi.rs
// Copyright © 2022-2026 Kyber SAS
// SPDX-License-Identifier: LicenseRef-Kyber-Commercial OR AGPL-3.0-or-later
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

#![allow(clippy::missing_safety_doc)]

use std::sync::Arc;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::AtomicPtr;

use log::{error, warn};
use std::io::BufReader;

use kynput_rs::Pipeline;

use crate::MetricsCallback;
use crate::VideoCodec;
use crate::get_resource_path;
use crate::metrics;
use crate::platform::VerifyMode;

pub type Display = super::Display;

pub struct WindowHandle {
    handle: crate::platform::WindowHandle,
}

pub struct ConnectConfig {
    config: crate::ConnectConfig,
}

pub struct AuthCredentials {
    inner: crate::AuthCredentials,
}

pub struct StreamingProtocol {
    protocol: crate::StreamingProtocol,
}

pub struct StreamingSessionConfig {
    config: crate::StreamingSessionConfig,
}

pub struct VideoPlayerConfig {
    config: crate::VideoPlayerConfig,
}

pub struct VideoSessionConfig {
    config: crate::VideoSessionConfig,
}

pub struct AudioSessionConfig {
    config: crate::AudioSessionConfig,
}

pub struct InputSessionConfig {
    config: crate::InputSessionConfig,
}

fn result_to_c_int<T>(result: crate::Result<T>) -> c_int {
    match result {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_init_crypto() {
    crate::init_crypto();
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum KymuxVideoProtocol {
    Reliable,
    GopStream,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<KymuxVideoProtocol> for kyctl_common::kymux::VideoProtocol {
    fn from(value: KymuxVideoProtocol) -> Self {
        match value {
            KymuxVideoProtocol::Reliable => Self::Reliable,
            KymuxVideoProtocol::GopStream => Self::GopStream,
            KymuxVideoProtocol::Unreliable => Self::Unreliable,
            KymuxVideoProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

#[cfg(feature = "kymux")]
#[derive(Clone, Copy)]
#[repr(C)]
pub enum KymuxAudioProtocol {
    Reliable,
    Unreliable,
    UnreliableFec,
}

#[cfg(feature = "kymux")]
impl From<KymuxAudioProtocol> for kyctl_common::kymux::AudioProtocol {
    fn from(value: KymuxAudioProtocol) -> Self {
        match value {
            KymuxAudioProtocol::Reliable => Self::Reliable,
            KymuxAudioProtocol::Unreliable => Self::Unreliable,
            KymuxAudioProtocol::UnreliableFec => Self::UnreliableFec,
        }
    }
}

pub use log4ky::capi::{LogCb, LogLevel};

/// # Safety
///
/// `cb` runs on any thread that logs, for as long as the process lives, so
/// `userdata` must stay valid and usable from several threads at once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_set_log_cb(cb: LogCb, userdata: *mut c_void) {
    let Some(logger) = (unsafe { log4ky::capi::CbLogger::new(cb, userdata) }) else {
        eprintln!("kyclient_set_log_cb: callback cannot be NULL");
        return;
    };
    let logger = Box::new(logger);

    if log::set_boxed_logger(logger).is_err() {
        return;
    }

    log::set_max_level(log::LevelFilter::Trace);
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_init_platform_verifier(
    env: std::ptr::NonNull<c_void>,
    context: std::ptr::NonNull<c_void>,
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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_set_android_logger() {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("kyclient"),
    );
}

/// Returns: (transfer full)
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_windowshandle_set_android_context(
    awindow_handler: *mut c_void,
) -> Box<WindowHandle> {
    log::info!(
        "kyclient_windowshandle_set_android_context {:?}",
        awindow_handler
    );
    Box::new(WindowHandle {
        handle: crate::platform::WindowHandle::Android(awindow_handler as u64),
    })
}

/// Returns: (transfer full)
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_windowshandle_from_x11(handle: u64) -> Box<WindowHandle> {
    Box::new(WindowHandle {
        handle: crate::platform::WindowHandle::Xlib(handle),
    })
}

/// Returns: (transfer full)
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_windowshandle_from_ns_view(handle: u64) -> Box<WindowHandle> {
    Box::new(WindowHandle {
        handle: crate::platform::WindowHandle::AppKit(handle),
    })
}

/// Returns: (transfer full)
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "visionos"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_windowshandle_from_ui_view(handle: u64) -> Box<WindowHandle> {
    Box::new(WindowHandle {
        handle: crate::platform::WindowHandle::UiKit(handle),
    })
}

/// Returns: (transfer full)
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_windowshandle_from_hwnd(hwnd: u64) -> Box<WindowHandle> {
    Box::new(WindowHandle {
        handle: crate::platform::WindowHandle::Win32(hwnd),
    })
}

/// Returns: (transfer full)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_auth_credentials_new_basic(
    username: *const c_char,
    password: *const c_char,
) -> Option<Box<AuthCredentials>> {
    unsafe {
        let Ok(username) = CStr::from_ptr(username).to_str() else {
            warn!("Fail to convert username");
            return None;
        };
        let username = username.to_owned();

        let Ok(password) = CStr::from_ptr(password).to_str() else {
            warn!("Fail to convert password");
            return None;
        };
        let password = password.to_owned();

        Some(Box::new(AuthCredentials {
            inner: crate::AuthCredentials::Basic { username, password },
        }))
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_auth_credentials_new_jwt(
    token: *const c_char,
) -> Option<Box<AuthCredentials>> {
    unsafe {
        let Ok(token) = CStr::from_ptr(token).to_str() else {
            warn!("Fail to convert token");
            return None;
        };
        let token = token.to_owned();

        Some(Box::new(AuthCredentials {
            inner: crate::AuthCredentials::Jwt { token },
        }))
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `token` must be a valid non-null null-terminated C string.
pub unsafe extern "C" fn kyclient_auth_credentials_new_oidc(
    token: *const c_char,
) -> Option<Box<AuthCredentials>> {
    unsafe {
        let Ok(token) = CStr::from_ptr(token).to_str() else {
            warn!("Failed to convert OIDC token");
            return None;
        };
        let token = token.to_owned();

        Some(Box::new(AuthCredentials {
            inner: crate::AuthCredentials::Oidc { token },
        }))
    }
}

/// @credentials: (transfer full)
/// @out_config: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_connect_config_default(
    host: *const c_char,
    credentials: Box<AuthCredentials>,
    out_config: *mut *mut ConnectConfig,
) -> c_int {
    unsafe {
        let Ok(host) = CStr::from_ptr(host).to_str() else {
            warn!("Fail to convert client Host");
            return -1;
        };

        let config = crate::ConnectConfig {
            host: host.into(),
            port: crate::STREAMER_PORT,
            credentials: credentials.inner,
            tls_host: None,
            verify_mode: None,
            auto_reconnection: false,
        };

        *out_config = Box::into_raw(Box::new(ConnectConfig { config }));
        0
    }
}

/// @credentials: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_set_credentials(
    config: &mut ConnectConfig,
    credentials: Box<AuthCredentials>,
) -> c_int {
    config.config.credentials = credentials.inner;

    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_host(
    config: &mut ConnectConfig,
    tls_host: *const c_char,
) -> c_int {
    unsafe {
        let Ok(tls_host) = CStr::from_ptr(tls_host).to_str() else {
            warn!("Fail to convert client TLS host");
            return -1;
        };

        config.config.tls_host = Some(tls_host.into());

        0
    }
}

#[unsafe(no_mangle)]
#[deprecated = "use kyclient_connect_config_set_tls_root_cert_path"]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_root_cert(
    config: &mut ConnectConfig,
    tls_root_cert: *const c_char,
) -> c_int {
    unsafe { kyclient_connect_config_set_tls_root_cert_path(config, tls_root_cert) }
}

/// Set TLS root certificate from a PEM file path.
///
/// This C API is preserved for backward compatibility with Android and iOS clients.
/// Internally converts to `VerifyMode::RootCert` - the old `tls_config` codepath
/// has been removed in favor of the unified `verify_mode` system.
///
/// The path is resolved relative to the executable directory if it's a relative path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_root_cert_path(
    config: &mut ConnectConfig,
    path: *const c_char,
) -> c_int {
    unsafe {
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

        config.config.verify_mode = Some(VerifyMode::RootCert { root_certificate });

        0
    }
}

/// Set TLS root certificate from raw PEM bytes.
///
/// This C API is preserved for backward compatibility with Android and iOS clients.
/// Internally converts to `VerifyMode::RootCert`.
/// @bytes: (array length=len)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_root_cert_bytes(
    config: &mut ConnectConfig,
    bytes: *const c_char,
    len: libc::size_t,
) -> c_int {
    unsafe {
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

        config.config.verify_mode = Some(VerifyMode::RootCert { root_certificate });

        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_set_streamer_port(config: &mut ConnectConfig, port: u16) {
    config.config.port = port;
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_set_auto_reconnection(
    config: &mut ConnectConfig,
    enabled: bool,
) {
    config.config.auto_reconnection = enabled;
}

/// @cfg: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_unref(cfg: Option<Box<ConnectConfig>>) {
    drop(cfg);
}

/// Set TLS certificate fingerprint for pinning verification.
/// The fingerprint should be in format "sha256:xx:xx:xx:..."
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_connect_config_set_tls_fingerprint(
    config: &mut ConnectConfig,
    fingerprint: *const c_char,
) -> c_int {
    unsafe {
        let Ok(fingerprint) = CStr::from_ptr(fingerprint).to_str() else {
            warn!("Fail to convert TLS fingerprint");
            return -1;
        };

        result_to_c_int(config.config.set_tls_fingerprint(fingerprint))
    }
}

/// Skip TLS verification (DANGER - for testing only)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_skip_tls_verification(config: &mut ConnectConfig) {
    config.config.skip_tls_verification();
}

/// TOFU prompt type
#[repr(C)]
pub enum TofuPromptType {
    UnknownHost,
    CertificateChanged,
}

/// TOFU prompt data passed to callback
#[repr(C)]
pub struct TofuPrompt {
    pub prompt_type: TofuPromptType,
    pub hostname: *const c_char,
    pub fingerprint: *const c_char,          // For UnknownHost
    pub expected_fingerprint: *const c_char, // For CertificateChanged
    pub actual_fingerprint: *const c_char,   // For CertificateChanged
}

/// TOFU callback type - returns true to accept, false to reject
pub type TofuCallback =
    Option<unsafe extern "C" fn(prompt: *const TofuPrompt, userdata: *mut c_void) -> bool>;

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
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_connect_config_enable_tofu(
    config: &mut ConnectConfig,
    callback: TofuCallback,
    userdata: *mut c_void,
) -> c_int {
    use crate::platform::tls;

    let Some(callback) = callback else {
        warn!("TOFU callback is null");
        return -1;
    };

    // Wrap the C callback in a Rust closure
    // SAFETY: userdata must remain valid for the lifetime of the connection
    let userdata = SendSyncPtr(userdata);
    let prompt_callback = move |prompt: tls::TofuPrompt| -> bool {
        let userdata = userdata.as_ptr();

        let (prompt_type, hostname_cstr, fingerprint_cstr, expected_cstr, actual_cstr) =
            match &prompt {
                tls::TofuPrompt::UnknownHost {
                    hostname,
                    fingerprint,
                } => {
                    let hostname_cstr = CString::new(hostname.as_str()).unwrap_or_default();
                    let fingerprint_cstr = CString::new(fingerprint.as_str()).unwrap_or_default();
                    (
                        TofuPromptType::UnknownHost,
                        hostname_cstr,
                        fingerprint_cstr,
                        CString::default(),
                        CString::default(),
                    )
                }
                tls::TofuPrompt::CertificateChanged {
                    hostname,
                    expected_fingerprint,
                    actual_fingerprint,
                } => {
                    let hostname_cstr = CString::new(hostname.as_str()).unwrap_or_default();
                    let expected_cstr =
                        CString::new(expected_fingerprint.as_str()).unwrap_or_default();
                    let actual_cstr = CString::new(actual_fingerprint.as_str()).unwrap_or_default();
                    (
                        TofuPromptType::CertificateChanged,
                        hostname_cstr,
                        CString::default(),
                        expected_cstr,
                        actual_cstr,
                    )
                }
            };

        let c_prompt = TofuPrompt {
            prompt_type,
            hostname: hostname_cstr.as_ptr(),
            fingerprint: fingerprint_cstr.as_ptr(),
            expected_fingerprint: expected_cstr.as_ptr(),
            actual_fingerprint: actual_cstr.as_ptr(),
        };

        unsafe { callback(&c_prompt, userdata) }
    };

    result_to_c_int(config.config.enable_tofu(prompt_callback))
}

/// Enable TOFU with auto-accept for new hosts
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_enable_tofu_auto_accept(
    config: &mut ConnectConfig,
) -> c_int {
    result_to_c_int(config.config.enable_tofu_auto_accept())
}

/// Enable strict known_hosts mode (reject unknown hosts)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_connect_config_enable_known_hosts_strict(
    config: &mut ConnectConfig,
) -> c_int {
    result_to_c_int(config.config.enable_known_hosts_strict())
}

/// Add a host to known_hosts file
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_known_hosts_add(
    hostname: *const c_char,
    fingerprint: *const c_char,
) -> c_int {
    unsafe {
        use crate::KnownHosts;

        let Ok(hostname) = CStr::from_ptr(hostname).to_str() else {
            warn!("Fail to convert hostname");
            return -1;
        };

        let Ok(fingerprint) = CStr::from_ptr(fingerprint).to_str() else {
            warn!("Fail to convert fingerprint");
            return -1;
        };

        let result = KnownHosts::load().and_then(|known_hosts| {
            known_hosts.insert(hostname, fingerprint)?;
            known_hosts.save()
        });

        result_to_c_int(result)
    }
}

/// Remove a host from known_hosts file
/// Returns 0 if removed, 1 if not found, -1 on error
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_known_hosts_remove(hostname: *const c_char) -> c_int {
    unsafe {
        use crate::KnownHosts;

        let Ok(hostname) = CStr::from_ptr(hostname).to_str() else {
            warn!("Fail to convert hostname");
            return -1;
        };

        let result = KnownHosts::load().and_then(|known_hosts| {
            let removed = known_hosts.remove(hostname)?;
            if removed {
                known_hosts.save()?;
            }
            Ok(removed)
        });

        match result {
            Ok(true) => 0,
            Ok(false) => 1,
            Err(_) => -1,
        }
    }
}

/// Known hosts entry for listing
#[repr(C)]
pub struct KnownHostEntry {
    pub hostname: *mut c_char,
    pub fingerprint: *mut c_char,
}

/// List all hosts in known_hosts file
/// Returns array of entries and count, caller must free with kyclient_known_hosts_list_free
/// @out_entries: (out) (array length=out_count)
/// @out_count: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_known_hosts_list(
    out_entries: *mut *mut KnownHostEntry,
    out_count: *mut usize,
) -> c_int {
    unsafe {
        use crate::KnownHosts;

        let hosts = match KnownHosts::load().and_then(|known_hosts| known_hosts.list()) {
            Ok(h) => h,
            Err(_) => return -1,
        };

        let count = hosts.len();
        if count == 0 {
            *out_entries = std::ptr::null_mut();
            *out_count = 0;
            return 0;
        }

        let entries =
            libc::malloc(count * std::mem::size_of::<KnownHostEntry>()) as *mut KnownHostEntry;
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
}

/// Free known hosts list returned by kyclient_known_hosts_list
/// @entries: (transfer full) (array length=count)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_known_hosts_list_free(
    entries: *mut KnownHostEntry,
    count: usize,
) {
    unsafe {
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
}

/// Returns: (transfer full)
#[cfg(feature = "rtp")]
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_protocol_rtp() -> Box<StreamingProtocol> {
    let protocol = crate::StreamingProtocol::Rtp;

    Box::new(StreamingProtocol { protocol })
}

/// Returns: (transfer full)
#[cfg(feature = "kymux")]
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_protocol_kymux(
    video_protocol: KymuxVideoProtocol,
    audio_protocol: KymuxAudioProtocol,
) -> Box<StreamingProtocol> {
    let protocol = crate::StreamingProtocol::Kymux(kyctl_common::kymux::KymuxConfig::KyProto(
        kyctl_common::kymux::KyProtoConfig {
            video_protocol: Some(video_protocol.into()),
            audio_protocol: Some(audio_protocol.into()),
        },
    ));

    Box::new(StreamingProtocol { protocol })
}

/// @protocol: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_protocol_unref(protocol: Option<Box<StreamingProtocol>>) {
    drop(protocol);
}

/// @cfg: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_player_config_unref(cfg: Option<Box<VideoPlayerConfig>>) {
    drop(cfg);
}

/// @window_handle: (transfer full)
/// @out_config: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_video_player_config_default(
    window_handle: Option<Box<WindowHandle>>,
    out_config: *mut *mut VideoPlayerConfig,
) -> c_int {
    unsafe {
        let Some(window_handle) = window_handle else {
            return -1;
        };

        let config = crate::VideoPlayerConfig {
            window_handle: window_handle.handle,
            software_decode: false,
            video_buffer: 0,
        };

        *out_config = Box::into_raw(Box::new(VideoPlayerConfig { config }));
        0
    }
}

/// @player: (transfer none)
/// @out_config: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_video_session_config_default(
    display_id: u32,
    player: *mut VideoPlayerConfig,
    out_config: *mut *mut VideoSessionConfig,
) -> c_int {
    unsafe {
        let config = crate::VideoSessionConfig {
            display_id,
            codec: VideoCodec::H264,
            yuv444: false,
            intra_refresh: None,
            player: (*player).config.clone(),
        };

        *out_config = Box::into_raw(Box::new(VideoSessionConfig { config }));
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_player_config_set_software_decode(
    config: &mut VideoPlayerConfig,
    software_decode: bool,
) {
    config.config.software_decode = software_decode;
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_player_config_set_video_buffer(
    config: &mut VideoPlayerConfig,
    video_buffer: u32,
) {
    config.config.video_buffer = video_buffer;
}

/// @cfg: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_session_config_unref(cfg: Option<Box<VideoSessionConfig>>) {
    drop(cfg);
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_session_config_set_yuv444(
    config: &mut VideoSessionConfig,
    yuv444: bool,
) {
    config.config.yuv444 = yuv444;
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_session_config_set_codec(
    config: &mut VideoSessionConfig,
    codec: VideoCodec,
) {
    config.config.codec = codec;
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_video_session_config_set_intra_refresh(
    config: &mut VideoSessionConfig,
    intra_refresh: bool,
) {
    config.config.intra_refresh = Some(intra_refresh);
}

/// @out_config: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_audio_session_config_default(
    out_config: *mut *mut AudioSessionConfig,
) -> c_int {
    unsafe {
        let config = crate::AudioSessionConfig { audio_buffer: None };
        *out_config = Box::into_raw(Box::new(AudioSessionConfig { config }));
        0
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_audio_session_config_set_audio_buffer(
    config: &mut AudioSessionConfig,
    audio_buffer: u32,
) {
    config.config.audio_buffer = (audio_buffer != 0).then_some(audio_buffer);
}

/// @out_config: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_input_session_config_default(
    pipeline: *mut kynput_sys::kynput_Pipeline,
    out_config: *mut *mut InputSessionConfig,
) -> c_int {
    unsafe {
        let pipeline = Pipeline::from_ptr(pipeline);

        let config = crate::InputSessionConfig {
            pipeline,
            gamepad_force_xbox360: None,
            clipboard: true,
        };

        *out_config = Box::into_raw(Box::new(InputSessionConfig { config }));
        0
    }
}

/// @cfg: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_input_session_config_unref(cfg: Option<Box<InputSessionConfig>>) {
    drop(cfg);
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_input_session_config_gamepad_force_xbox360(
    cfg: &mut InputSessionConfig,
    gamepad_force_xbox360: bool,
) {
    cfg.config.gamepad_force_xbox360 = Some(gamepad_force_xbox360);
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_input_session_config_clipboard(
    cfg: &mut InputSessionConfig,
    enable: bool,
) {
    cfg.config.clipboard = enable;
}

/// @out_config: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_streaming_session_config_default(
    protocol: &StreamingProtocol,
    out_config: *mut *mut StreamingSessionConfig,
) -> c_int {
    unsafe {
        let config = crate::StreamingSessionConfig {
            protocol: protocol.protocol.clone(),
            bitrate: 20_000_000,
            video: vec![],
            audio: None,
            input: None,
        };

        *out_config = Box::into_raw(Box::new(StreamingSessionConfig { config }));
        0
    }
}

/// @cfg: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_session_config_unref(
    cfg: Option<Box<StreamingSessionConfig>>,
) {
    drop(cfg);
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_session_config_set_bitrate(
    config: &mut StreamingSessionConfig,
    bitrate: u32,
) {
    config.config.bitrate = bitrate;
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_session_config_get_bitrate(
    config: &mut StreamingSessionConfig,
) -> u32 {
    config.config.bitrate
}

/// @audio: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_session_config_set_audio(
    config: &mut StreamingSessionConfig,
    audio: Box<AudioSessionConfig>,
) {
    config.config.audio = Some(audio.config);
}

/// @video: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_session_config_add_video(
    config: &mut StreamingSessionConfig,
    video: Box<VideoSessionConfig>,
) {
    config.config.video.push(video.config);
}

/// @input: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_streaming_session_config_set_input(
    config: &mut StreamingSessionConfig,
    input: Box<InputSessionConfig>,
) {
    config.config.input = Some(input.config);
}

pub struct Client {
    client: crate::Client,
}

/// Returns: (transfer full)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_new(
    connect_config: *mut ConnectConfig,
    metrics: bool,
) -> Option<Box<Client>> {
    unsafe {
        let connect_config = (*connect_config).config.clone();

        let client = crate::Client::new(connect_config, metrics);
        if let Ok(client) = client {
            Some(Box::new(Client { client }))
        } else {
            None
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_stop(client: *mut Client) -> c_int {
    unsafe {
        let ret = (&(*client)).client.stop();
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `client` must be a valid non-null pointer. `credentials` must be a pointer to a
/// `AuthCredentials` allocated by this library; ownership is transferred and
/// the caller must not use it after this call.
pub unsafe extern "C" fn kyclient_client_update_credentials(
    client: *mut Client,
    credentials: Box<AuthCredentials>,
) -> c_int {
    unsafe {
        let ret = (*client).client.update_credentials(credentials.inner);
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `client` must be a valid non-null pointer. `credentials` must be a pointer to a
/// `AuthCredentials` allocated by this library; ownership is transferred and
/// the caller must not use it after this call.
pub unsafe extern "C" fn kyclient_client_renew_session(
    client: *mut Client,
    credentials: Box<AuthCredentials>,
) -> c_int {
    unsafe {
        let ret = (*client).client.renew_session(credentials.inner);
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_disconnect_and_stop(
    client: *mut Client,
    stop_session: bool,
) -> c_int {
    unsafe {
        let ret = (&(*client)).client.disconnect_and_stop(stop_session);
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_connect(client: *mut Client) -> c_int {
    unsafe {
        let ret = (&(*client)).client.connect();
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_disconnect(client: *mut Client) -> c_int {
    unsafe {
        let ret = (&(*client)).client.disconnect();
        result_to_c_int(ret)
    }
}

/// @session_config: (transfer none)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_start_streaming(
    client: *mut Client,
    session_config: *mut StreamingSessionConfig,
) -> c_int {
    unsafe {
        let session_config = (*session_config).config.clone();

        let ret = (&(*client)).client.start_streaming(session_config);
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_stop_streaming(client: *mut Client) -> c_int {
    unsafe {
        let ret = (&(*client)).client.stop_streaming();
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_audio_set_volume(
    client: *mut Client,
    volume: f32,
) -> c_int {
    unsafe {
        let ret = (&(*client)).client.audio_set_volume(volume);
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_audio_set_mute(client: *mut Client, mute: bool) -> c_int {
    unsafe {
        let ret = (&(*client)).client.audio_set_mute(mute);
        result_to_c_int(ret)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_set_bitrate(client: *mut Client, bitrate: u32) -> c_int {
    unsafe {
        let ret = (&(*client)).client.set_bitrate(bitrate);
        result_to_c_int(ret)
    }
}

/// @client: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_client_unref(client: Box<Client>) {
    drop(client);
}

pub type Event = super::Event;

#[repr(C)]
pub enum EventType {
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
pub enum ConnectFailedReason {
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
pub enum StopReason {
    Requested,
    ControlPlaneClosed,
    ControlPlaneIoError,
    ControlPlaneTimeout,
    DecodeError,
}

#[repr(C)]
pub enum StreamerPipelineType {
    EnumerateDisplays,
    Audio,
    Video,
}

#[repr(C)]
pub enum StreamerPipelineStage {
    Process,
    Initialization,
    Acquisition,
    Encoder,
}

#[repr(C)]
pub enum StreamerPipelineEventType {
    Stopped,
    Error,
}

#[unsafe(no_mangle)]
pub extern "C" fn kyclient_event_get_type(event: &Event) -> EventType {
    match event {
        Event::ControlPlaneConnected { .. } => EventType::ControlPlaneConnected,
        Event::ControlPlaneConnectFailed { .. } => EventType::ControlPlaneConnectFailed,
        Event::AuthenticationRequired { .. } => EventType::AuthenticationRequired,
        Event::DataPlaneConnectFailed => EventType::DataPlaneConnectFailed,
        Event::DataPlaneClosed => EventType::DataPlaneClosed,
        Event::Reconnecting { .. } => EventType::Reconnecting,
        Event::Reconnected { .. } => EventType::Reconnected,
        Event::ReconnectionFailed => EventType::ReconnectionFailed,
        Event::Stopped { .. } => EventType::Stopped,
        Event::DisplayListUpdated { .. } => EventType::DisplayListUpdated,
        Event::InputServiceConnected => EventType::InputServiceConnected,
        Event::InputServiceDisconnected => EventType::InputServiceDisconnected,
        Event::StreamingStarted => EventType::StreamingStarted,
        Event::StreamingStartFailed => EventType::StreamingStartFailed,
        Event::StreamingStopped => EventType::StreamingStopped,
        Event::StreamerPipelineEvent { .. } => EventType::StreamerPipelineEvent,
        Event::AudioVolumeChanged { .. } => EventType::AudioVolumeChanged,
        Event::AudioMuteChanged { .. } => EventType::AudioMuteChanged,
        Event::BitrateChanged { .. } => EventType::BitrateChanged,
    }
}

/// @out_displays: (out) (array length=out_len)
/// @out_len: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_control_plane_connected(
    event: &Event,
    out_displays: *mut *mut Display,
    out_len: *mut u32,
) -> c_int {
    unsafe {
        if let Event::ControlPlaneConnected { displays } = event {
            let mut displays = Box::new(displays.clone());

            *out_displays = displays.as_mut_ptr();
            *out_len = displays.len() as u32;

            std::mem::forget(displays);

            0
        } else {
            -1
        }
    }
}

/// @out_reason: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_connect_failed(
    event: &Event,
    out_reason: *mut ConnectFailedReason,
) -> c_int {
    unsafe {
        let Event::ControlPlaneConnectFailed { reason } = event else {
            return -1;
        };

        *out_reason = match reason {
            crate::ConnectFailedReason::NetworkError => ConnectFailedReason::NetworkError,
            crate::ConnectFailedReason::Timeout => ConnectFailedReason::Timeout,
            crate::ConnectFailedReason::Unauthorized => ConnectFailedReason::Unauthorized,
            crate::ConnectFailedReason::ServerError => ConnectFailedReason::ServerError,
            crate::ConnectFailedReason::TlsVerification => ConnectFailedReason::TlsVerification,
            crate::ConnectFailedReason::TlsConfiguration => ConnectFailedReason::TlsConfiguration,
            crate::ConnectFailedReason::WebSocket => ConnectFailedReason::WebSocket,
            crate::ConnectFailedReason::Protocol => ConnectFailedReason::Protocol,
        };

        0
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `event` must be a valid `ControlPlaneAuthenticationRequired` event. On success,
/// `out_issuer` and `out_client_id` receive freshly-allocated C strings that the
/// caller must free with `kyclient_event_free_authentication_required`.
pub unsafe extern "C" fn kyclient_event_unpack_authentication_required(
    event: *const Event,
    out_issuer: *mut *mut c_char,
    out_client_id: *mut *mut c_char,
) -> c_int {
    unsafe {
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
}

#[unsafe(no_mangle)]
/// # Safety
/// `issuer` and `client_id` must be pointers returned by
/// `kyclient_event_unpack_authentication_required`, and must not be freed more than once.
pub unsafe extern "C" fn kyclient_event_free_authentication_required(
    issuer: *mut c_char,
    client_id: *mut c_char,
) {
    unsafe {
        if !issuer.is_null() {
            let _ = CString::from_raw(issuer);
        }
        if !client_id.is_null() {
            let _ = CString::from_raw(client_id);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_stopped(
    event: &Event,
    out_reason: *mut StopReason,
) -> c_int {
    unsafe {
        let Event::Stopped { reason } = event else {
            return -1;
        };

        *out_reason = match reason {
            crate::StopReason::Requested => StopReason::Requested,
            crate::StopReason::ControlPlaneClosed => StopReason::ControlPlaneClosed,
            crate::StopReason::ControlPlaneIoError => StopReason::ControlPlaneIoError,
            crate::StopReason::ControlPlaneTimeout => StopReason::ControlPlaneTimeout,
            crate::StopReason::DecodeError => StopReason::DecodeError,
        };

        0
    }
}

/// @out_attempt: (out)
/// @out_max_attempts: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_reconnecting(
    event: &Event,
    out_attempt: *mut u32,
    out_max_attempts: *mut u32,
) -> c_int {
    unsafe {
        let Event::Reconnecting {
            attempt,
            max_attempts,
        } = event
        else {
            return -1;
        };

        *out_attempt = *attempt;
        *out_max_attempts = *max_attempts;

        0
    }
}

/// @out_should_restart_streaming: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_reconnected(
    event: &Event,
    out_should_restart_streaming: *mut bool,
) -> c_int {
    unsafe {
        let Event::Reconnected {
            should_restart_streaming,
        } = event
        else {
            return -1;
        };

        *out_should_restart_streaming = *should_restart_streaming;

        0
    }
}

/// @out_displays: (out) (array length=out_len)
/// @out_len: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_display_list_updated(
    event: &Event,
    out_displays: *mut *mut Display,
    out_len: *mut u32,
) -> c_int {
    unsafe {
        if let Event::DisplayListUpdated { displays } = event {
            let mut displays = Box::new(displays.clone());

            *out_displays = displays.as_mut_ptr();
            *out_len = displays.len() as u32;

            std::mem::forget(displays);

            0
        } else {
            -1
        }
    }
}

/// @displays: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_event_free_displays(displays: Box<Display>) {
    drop(displays);
}

/// @out_type: (out)
/// @out_stage: (out)
/// @out_event: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_streamer_pipeline_event(
    event: &Event,
    out_type: *mut StreamerPipelineType,
    out_stage: *mut StreamerPipelineStage,
    out_event: *mut StreamerPipelineEventType,
) -> c_int {
    unsafe {
        let Event::StreamerPipelineEvent {
            type_,
            stage,
            event,
        } = event
        else {
            return -1;
        };

        if out_type.is_null() || out_stage.is_null() || out_event.is_null() {
            return -1;
        }

        *out_type = match type_ {
            crate::StreamerPipelineType::EnumerateDisplays => {
                StreamerPipelineType::EnumerateDisplays
            }
            crate::StreamerPipelineType::Audio => StreamerPipelineType::Audio,
            crate::StreamerPipelineType::Video => StreamerPipelineType::Video,
        };

        *out_stage = match stage {
            crate::StreamerPipelineStage::Process => StreamerPipelineStage::Process,
            crate::StreamerPipelineStage::Initialization => StreamerPipelineStage::Initialization,
            crate::StreamerPipelineStage::Acquisition => StreamerPipelineStage::Acquisition,
            crate::StreamerPipelineStage::Encoder => StreamerPipelineStage::Encoder,
        };

        *out_event = match event {
            crate::StreamerPipelineEventType::Stopped => StreamerPipelineEventType::Stopped,
            crate::StreamerPipelineEventType::Error => StreamerPipelineEventType::Error,
        };

        0
    }
}

/// @out_volume: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_audio_volume_changed(
    event: &Event,
    out_volume: *mut f32,
) -> c_int {
    unsafe {
        let Event::AudioVolumeChanged { volume } = event else {
            return -1;
        };

        *out_volume = *volume;

        0
    }
}

/// @out_mute: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_audio_mute_changed(
    event: &Event,
    out_mute: *mut bool,
) -> c_int {
    unsafe {
        let Event::AudioMuteChanged { mute } = &event else {
            return -1;
        };

        *out_mute = *mute;

        0
    }
}

/// @out_bitrate: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_event_unpack_bitrate_changed(
    event: &Event,
    out_bitrate: *mut u32,
) -> c_int {
    unsafe {
        let Event::BitrateChanged { bitrate } = event else {
            return -1;
        };

        *out_bitrate = *bitrate;

        0
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventHandler {
    on_event: Option<extern "C" fn(event: *const Event, userdata: *mut c_void)>,
    userdata: *mut c_void,
}

unsafe impl Send for EventHandler {}
unsafe impl Sync for EventHandler {}

impl crate::event_manager::EventSink for EventHandler {
    fn on_event(&self, event: &crate::Event) {
        if let Some(on_event) = self.on_event {
            on_event(event, self.userdata);
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_set_event_handler(
    client: *mut Client,
    handler: EventHandler,
) -> c_int {
    unsafe {
        (&(*client))
            .client
            .event_manager
            .add_event_sink(Box::new(handler));

        0
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum MetricType {
    Video,
    NetworkLocal,
    NetworkPing,
    NetworkRemote,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct VideoMetric {
    pub source_id: u32,
    pub pts: u64,
    pub ts: i64,
    pub data: VideoMetricData,
}

impl From<metrics::VideoMetric> for VideoMetric {
    fn from(video_metric: metrics::VideoMetric) -> Self {
        Self {
            source_id: video_metric.source_id,
            pts: video_metric.pts,
            ts: video_metric.ts,
            data: video_metric.data.into(),
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub enum VideoMetricData {
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

impl From<metrics::VideoMetricData> for VideoMetricData {
    fn from(data: metrics::VideoMetricData) -> Self {
        match data {
            metrics::VideoMetricData::FrameAcquired => Self::FrameAcquired,
            metrics::VideoMetricData::FrameEncoding => Self::FrameEncoding,
            metrics::VideoMetricData::FrameEncoded => Self::FrameEncoded,
            metrics::VideoMetricData::FrameSent => Self::FrameSent,
            metrics::VideoMetricData::FrameReceived => Self::FrameReceived,
            metrics::VideoMetricData::FrameDecoding => Self::FrameDecoding,
            metrics::VideoMetricData::FrameDecoded => Self::FrameDecoded,
            metrics::VideoMetricData::FramePrepared => Self::FramePrepared,
            metrics::VideoMetricData::FrameDisplayed => Self::FrameDisplayed,
            metrics::VideoMetricData::FrameSkipped => Self::FrameSkipped,
        }
    }
}

impl From<VideoMetricData> for metrics::VideoMetricData {
    fn from(data: VideoMetricData) -> Self {
        match data {
            VideoMetricData::FrameAcquired => Self::FrameAcquired,
            VideoMetricData::FrameEncoding => Self::FrameEncoding,
            VideoMetricData::FrameEncoded => Self::FrameEncoded,
            VideoMetricData::FrameSent => Self::FrameSent,
            VideoMetricData::FrameReceived => Self::FrameReceived,
            VideoMetricData::FrameDecoding => Self::FrameDecoding,
            VideoMetricData::FrameDecoded => Self::FrameDecoded,
            VideoMetricData::FramePrepared => Self::FramePrepared,
            VideoMetricData::FrameDisplayed => Self::FrameDisplayed,
            VideoMetricData::FrameSkipped => Self::FrameSkipped,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct NetworkStats {
    pub rtt_micros: i64,
    pub packets_lost: i64,
    pub dropped_packets: i64,
}

impl From<metrics::NetworkStats> for NetworkStats {
    fn from(network_stats: metrics::NetworkStats) -> Self {
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

#[repr(C)]
#[derive(Copy, Clone)]
pub struct NetworkPing {
    pub offset_micros: i64,
    pub delay_micros: i64,
}

impl From<metrics::NetworkPing> for NetworkPing {
    fn from(network_ping: metrics::NetworkPing) -> Self {
        Self {
            offset_micros: network_ping.offset_micros,
            delay_micros: network_ping.delay_micros,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union MetricData {
    pub video: VideoMetric,
    pub network_local: NetworkStats,
    pub network_ping: NetworkPing,
    pub network_remote: NetworkStats,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Metric {
    pub type_: MetricType,
    pub data: MetricData,
}

impl From<metrics::Metric> for Metric {
    fn from(metric: metrics::Metric) -> Self {
        match metric {
            metrics::Metric::Video(video_metric) => Self {
                type_: MetricType::Video,
                data: MetricData {
                    video: video_metric.into(),
                },
            },
            metrics::Metric::NetworkLocal(network_stats) => Self {
                type_: MetricType::NetworkLocal,
                data: MetricData {
                    network_local: network_stats.into(),
                },
            },
            metrics::Metric::NetworkPing(network_ping) => Self {
                type_: MetricType::NetworkPing,
                data: MetricData {
                    network_ping: network_ping.into(),
                },
            },
            metrics::Metric::NetworkRemote(network_stats) => Self {
                type_: MetricType::NetworkRemote,
                data: MetricData {
                    network_remote: network_stats.into(),
                },
            },
        }
    }
}

pub type MetricsCb = Option<extern "C" fn(metric: *const Metric, userdata: *mut c_void)>;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_client_set_metrics_callback(
    client: *mut Client,
    metrics_cb: MetricsCb,
    userdata: *mut c_void,
) {
    let cb: Option<Box<MetricsCallback>> = if let Some(metrics_cb) = metrics_cb {
        let metrics_cb = Arc::new(metrics_cb);
        let userdata = Arc::new(AtomicPtr::new(userdata));
        let cb = move |metric: metrics::Metric| unsafe {
            let userdata = *userdata.as_ptr();
            let kyclient_metric: Metric = metric.into();
            metrics_cb(std::ptr::from_ref(&kyclient_metric), userdata);
        };

        Some(Box::new(cb))
    } else {
        None
    };

    let client = unsafe { &(*client).client };
    client.set_metrics_callback(cb);
}

pub struct MetricsJsonProducer {
    producer: metrics::MetricsJSONProducer,
}

pub struct MetricsJson {
    _data: Vec<CString>,
    c_data: Vec<*const c_char>,
}

/// Returns: (transfer full)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_metrics_json_producer_new() -> Box<MetricsJsonProducer> {
    let producer = metrics::MetricsJSONProducer::new();
    Box::new(MetricsJsonProducer { producer })
}

/// @producer: (transfer full)
#[unsafe(no_mangle)]
pub extern "C" fn kyclient_metrics_json_producer_unref(producer: Box<MetricsJsonProducer>) {
    drop(producer);
}

/// @metric: (transfer none)
/// Returns: (transfer full)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_metrics_json_producer_push(
    producer: &mut MetricsJsonProducer,
    metric: &Metric,
) -> Box<MetricsJson> {
    unsafe {
        let map_value = |v: i64| -> Option<u64> { if v >= 0 { Some(v as u64) } else { None } };

        let metric = match metric.type_ {
            MetricType::Video => {
                let data = &metric.data.video;

                metrics::Metric::Video(metrics::VideoMetric {
                    source_id: data.source_id,
                    pts: data.pts,
                    ts: data.ts,
                    data: data.data.into(),
                })
            }
            MetricType::NetworkLocal => {
                let data = &metric.data.network_local;

                metrics::Metric::NetworkLocal(metrics::NetworkStats {
                    rtt_micros: map_value(data.rtt_micros),
                    packets_lost: map_value(data.packets_lost),
                    dropped_packets: map_value(data.dropped_packets),
                })
            }
            MetricType::NetworkPing => {
                let data = &metric.data.network_ping;

                metrics::Metric::NetworkPing(metrics::NetworkPing {
                    offset_micros: data.offset_micros,
                    delay_micros: data.delay_micros,
                })
            }
            MetricType::NetworkRemote => {
                let data = &metric.data.network_remote;

                metrics::Metric::NetworkRemote(metrics::NetworkStats {
                    rtt_micros: map_value(data.rtt_micros),
                    packets_lost: map_value(data.packets_lost),
                    dropped_packets: map_value(data.dropped_packets),
                })
            }
        };

        let json = producer.producer.push(&metric);

        let mut data = Vec::with_capacity(json.len());

        for l in json {
            let s = CString::from_vec_unchecked(l);
            data.push(s);
        }

        let c_data = data.iter().map(|s| s.as_ptr()).collect::<Vec<_>>();

        Box::new(MetricsJson {
            _data: data,
            c_data,
        })
    }
}

/// @out_values: (out) (array length=out_count)
/// @out_count: (out)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_metrics_json_values(
    json: *mut MetricsJson,
    out_values: *mut *const *const c_char,
    out_count: *mut libc::size_t,
) {
    unsafe {
        let json = &*json;

        *out_values = json.c_data.as_ptr();
        *out_count = json.c_data.len();
    }
}

/// @json: (transfer full)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kyclient_metrics_json_unref(json: Box<MetricsJson>) {
    drop(json);
}
