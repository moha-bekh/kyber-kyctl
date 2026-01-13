// Project Kyber: windows.rs
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

//! Windows-specific system tray implementation.
//!
//! This module contains all Windows-specific code for the system tray,
//! including the message pump, tray icon creation, and Windows service integration.
//!
//! # Native Shell_NotifyIconW Implementation
//!
//! This module uses direct Windows Shell_NotifyIconW API calls with NIF_GUID
//! to enable cross-process icon identification. This allows:
//! - New controller instances to clean up orphan icons from crashed instances
//! - Reliable icon removal regardless of how the process exits
//!
//! The tray-icon crate is NOT used for icon management because it creates icons
//! without NIF_GUID, making orphan cleanup impossible.

use super::super::status::{TrayCommand, TrayStatus};
use crate::error::{Error, Result};

use log::{info, warn};
use muda::{ContextMenu, Menu};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::thread::JoinHandle;
use tokio::sync::mpsc;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::System::Threading::SetEvent;
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow,
    GetWindowLongPtrW, RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, CREATESTRUCTW,
    CW_USEDEFAULT, GWLP_USERDATA, HICON, ICONINFO, WM_LBUTTONUP, WM_NCCREATE, WM_RBUTTONUP,
    WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

/// Wrapper for Windows HANDLE that implements Send + Sync and auto-closes on drop.
///
/// Windows event handles are safe to use across threads (they are kernel objects
/// with reference counting), but raw pointers don't implement Send/Sync.
struct SendHandle(windows_sys::Win32::Foundation::HANDLE);

unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

impl SendHandle {
    fn as_raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }

    fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl Drop for SendHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Safety: handle is valid and we own it
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Handle to control the system tray from async code.
///
/// All methods are async-safe despite the tray running on a dedicated thread.
/// Communication uses Windows events for shutdown and tokio channels for status.
pub struct TrayHandle {
    /// Send status updates to the tray thread.
    status_tx: mpsc::Sender<TrayStatus>,
    /// Windows event for shutdown signaling.
    shutdown_event: SendHandle,
    /// Windows event for waking to check status updates.
    wake_event: SendHandle,
    /// Join handle for the tray thread.
    thread_handle: Option<JoinHandle<()>>,
}

impl TrayHandle {
    /// Create a new TrayHandle.
    fn new(
        status_tx: mpsc::Sender<TrayStatus>,
        shutdown_event: SendHandle,
        wake_event: SendHandle,
        thread_handle: JoinHandle<()>,
    ) -> Self {
        Self {
            status_tx,
            shutdown_event,
            wake_event,
            thread_handle: Some(thread_handle),
        }
    }

    /// Update the tray icon status (tooltip and potentially icon).
    ///
    /// This is async and will not block the caller.
    pub async fn set_status(&self, status: TrayStatus) {
        if let Err(e) = self.status_tx.send(status).await {
            warn!("Failed to send status to tray: {e}");
            return;
        }

        // Signal the wake event to notify the tray thread
        // Safety: wake_event is a valid handle created by CreateEventW
        unsafe { SetEvent(self.wake_event.as_raw()) };
    }

    /// Shutdown the tray thread and wait for it to finish.
    ///
    /// This ensures the tray icon is properly removed before process exit.
    pub async fn shutdown(&mut self) {
        info!("Shutting down tray...");

        // Signal the tray thread to exit
        // Safety: shutdown_event is a valid handle created by CreateEventW
        unsafe { SetEvent(self.shutdown_event.as_raw()) };

        // Wait for the thread to finish
        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(()) => info!("Tray thread exited cleanly"),
                Err(_) => warn!("Tray thread panicked during shutdown"),
            }
        }
    }
}

impl Drop for TrayHandle {
    fn drop(&mut self) {
        // Signal the tray thread to exit if not already done
        if !self.shutdown_event.is_null() {
            // Safety: shutdown_event is a valid handle created by CreateEventW
            unsafe { SetEvent(self.shutdown_event.as_raw()) };
        }

        // Wait for the thread to finish (blocking, but Drop is synchronous)
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        // Event handles are closed automatically by SendHandle::drop()
    }
}

/// Embedded Kyber icon (PNG format).
const ICON_BYTES: &[u8] = include_bytes!("../../../res/kyber_tray.png");

/// Menu item IDs
const MENU_STOP_ID: &str = "stop";
const MENU_RESTART_ID: &str = "restart";

/// Custom message for tray icon callbacks.
const WM_TRAYICON: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_USER + 1;

/// Window class name for the hidden tray window.
const TRAY_WINDOW_CLASS: &str = "KyberTrayWindow";

/// Fixed GUID for the tray icon.
/// Using NIF_GUID allows Windows to identify the icon across process restarts,
/// enabling cleanup of orphan icons when a new controller starts.
/// GUID: {7B3F8E2A-1C4D-4E5F-9A6B-8C7D0E1F2A3B}
const TRAY_ICON_GUID: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x7B3F8E2A,
    data2: 0x1C4D,
    data3: 0x4E5F,
    data4: [0x9A, 0x6B, 0x8C, 0x7D, 0x0E, 0x1F, 0x2A, 0x3B],
};

/// Wait result from the tray event loop.
enum WaitResult {
    /// Shutdown event was signaled.
    Shutdown,
    /// Wake event was signaled (status update available).
    Wake,
    /// Windows messages are available.
    WindowsMessage,
}

// ============================================================================
// Native Tray Icon Implementation using Shell_NotifyIconW with NIF_GUID
// ============================================================================

// Thread-local storage for menu and window handle (needed by window procedure).
thread_local! {
    static TRAY_CONTEXT: std::cell::RefCell<Option<TrayContext>> = const { std::cell::RefCell::new(None) };
}

/// Context for the tray window procedure to access menu and window handle.
struct TrayContext {
    menu: Menu,
    hwnd: HWND,
}

/// Window procedure for the hidden tray window.
///
/// Handles WM_TRAYICON messages to show the context menu using muda's show_context_menu_for_hwnd.
unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_TRAYICON {
        let event = (lparam & 0xFFFF) as u32;
        if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
            // Show context menu using muda's method (handles WM_COMMAND properly)
            TRAY_CONTEXT.with(|ctx| {
                if let Some(ref ctx) = *ctx.borrow() {
                    // Required to make menu dismiss when clicking elsewhere
                    SetForegroundWindow(hwnd);
                    // Use muda's show_context_menu_for_hwnd - pass None to use cursor position
                    ctx.menu.show_context_menu_for_hwnd(ctx.hwnd as isize, None);
                }
            });
        }
        return 0;
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Native tray icon using Shell_NotifyIconW with NIF_GUID.
///
/// This implementation creates icons with a fixed GUID, allowing:
/// - Cleanup of orphan icons from previous crashed instances
/// - Reliable icon identification across process restarts
struct NativeTrayIcon {
    hwnd: HWND,
    hicon: HICON,
}

impl NativeTrayIcon {
    /// Create a new native tray icon.
    ///
    /// This also attempts to remove any orphan icon with the same GUID first.
    fn new(icon_data: &[u8], tooltip: &str) -> Result<Self> {
        // First, try to remove any orphan icon with our GUID
        remove_orphan_icon();

        // Register window class (ignore error if already registered)
        let class_name = to_wide_string(TRAY_WINDOW_CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(tray_window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: std::ptr::null_mut(),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        unsafe { RegisterClassW(&wc) };

        // Create hidden window for receiving messages
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return Err(Error::TrayInit {
                reason: "Failed to create hidden window".to_string(),
            });
        }

        // Create HICON from PNG data
        let hicon = create_hicon_from_png(icon_data)?;

        // Create the tray icon
        let mut nid = create_notify_icon_data(hwnd, hicon, tooltip);
        nid.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE | NIF_GUID;

        let result = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };
        if result == 0 {
            unsafe {
                DestroyIcon(hicon);
                DestroyWindow(hwnd);
            }
            return Err(Error::TrayInit {
                reason: "Shell_NotifyIconW(NIM_ADD) failed".to_string(),
            });
        }

        Ok(Self { hwnd, hicon })
    }

    /// Update the tooltip text.
    fn set_tooltip(&self, tooltip: &str) {
        let mut nid = create_notify_icon_data(self.hwnd, self.hicon, tooltip);
        nid.uFlags = NIF_TIP | NIF_GUID;

        unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };
    }

    /// Get the hidden window handle.
    fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl Drop for NativeTrayIcon {
    fn drop(&mut self) {
        // Remove the tray icon using GUID
        let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.uFlags = NIF_GUID;
        nid.guidItem = TRAY_ICON_GUID;
        unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };

        if !self.hicon.is_null() {
            unsafe { DestroyIcon(self.hicon) };
        }
        if !self.hwnd.is_null() {
            unsafe { DestroyWindow(self.hwnd) };
        }
    }
}

/// Convert a Rust string to a null-terminated wide string.
fn to_wide_string(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Create a NOTIFYICONDATAW structure with common fields filled.
fn create_notify_icon_data(hwnd: HWND, hicon: HICON, tooltip: &str) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 0; // Not used when NIF_GUID is set
    nid.uCallbackMessage = WM_TRAYICON;
    nid.hIcon = hicon;
    nid.guidItem = TRAY_ICON_GUID;

    // Copy tooltip (max 127 chars + null terminator)
    let tooltip_wide = to_wide_string(tooltip);
    let copy_len = tooltip_wide.len().min(nid.szTip.len());
    nid.szTip[..copy_len].copy_from_slice(&tooltip_wide[..copy_len]);

    nid
}

/// Create an HICON from PNG data using CreateIconIndirect.
fn create_hicon_from_png(png_data: &[u8]) -> Result<HICON> {
    // Decode PNG
    let decoder = png::Decoder::new(png_data);
    let mut reader = decoder.read_info().map_err(|e| Error::TrayIconLoad {
        reason: e.to_string(),
    })?;

    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| Error::TrayIconLoad {
            reason: e.to_string(),
        })?;

    // Convert to RGBA if needed
    let rgba_data = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            // Add alpha channel (fully opaque) to RGB pixels
            let rgb_data = &buf[..info.buffer_size()];
            let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
            for chunk in rgb_data.chunks(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        other => {
            return Err(Error::TrayIconLoad {
                reason: format!("Unsupported color type: {:?}", other),
            });
        }
    };

    let width = info.width as i32;
    let height = info.height as i32;

    // Convert RGBA to BGRA (Windows DIB format) with pre-multiplied alpha
    let mut bgra_data = Vec::with_capacity(rgba_data.len());
    for chunk in rgba_data.chunks(4) {
        let (r, g, b, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        // Pre-multiply alpha for proper transparency
        let alpha = a as u32;
        let premul_r = ((r as u32 * alpha) / 255) as u8;
        let premul_g = ((g as u32 * alpha) / 255) as u8;
        let premul_b = ((b as u32 * alpha) / 255) as u8;
        bgra_data.push(premul_b); // Blue
        bgra_data.push(premul_g); // Green
        bgra_data.push(premul_r); // Red
        bgra_data.push(a); // Alpha
    }

    // Create a compatible DC
    let hdc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    if hdc.is_null() {
        return Err(Error::TrayIconLoad {
            reason: "CreateCompatibleDC failed".to_string(),
        });
    }

    // Set up BITMAPINFO for a 32-bit top-down DIB
    let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
    bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bmi.bmiHeader.biWidth = width;
    bmi.bmiHeader.biHeight = -height; // Negative for top-down
    bmi.bmiHeader.biPlanes = 1;
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = BI_RGB;

    // Create the color bitmap as a DIB section
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbm_color = unsafe {
        CreateDIBSection(
            hdc,
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        )
    };
    if hbm_color.is_null() {
        unsafe { DeleteDC(hdc) };
        return Err(Error::TrayIconLoad {
            reason: "CreateDIBSection failed".to_string(),
        });
    }

    // Copy the BGRA data to the DIB section
    if !bits.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(bgra_data.as_ptr(), bits as *mut u8, bgra_data.len())
        };
    }

    // Create a monochrome mask bitmap (all zeros = fully opaque when combined with 32-bit color)
    let hbm_mask = unsafe { CreateBitmap(width, height, 1, 1, std::ptr::null()) };
    if hbm_mask.is_null() {
        unsafe {
            DeleteObject(hbm_color as _);
            DeleteDC(hdc);
        }
        return Err(Error::TrayIconLoad {
            reason: "CreateBitmap for mask failed".to_string(),
        });
    }

    // Create the icon using ICONINFO
    let mut icon_info: ICONINFO = unsafe { std::mem::zeroed() };
    icon_info.fIcon = 1; // TRUE = icon (not cursor)
    icon_info.xHotspot = 0;
    icon_info.yHotspot = 0;
    icon_info.hbmMask = hbm_mask;
    icon_info.hbmColor = hbm_color;

    let hicon = unsafe { CreateIconIndirect(&icon_info) };

    // Clean up bitmaps and DC (CreateIconIndirect makes copies)
    unsafe {
        DeleteObject(hbm_mask as _);
        DeleteObject(hbm_color as _);
        DeleteDC(hdc);
    }

    if hicon.is_null() {
        return Err(Error::TrayIconLoad {
            reason: "CreateIconIndirect failed".to_string(),
        });
    }

    Ok(hicon)
}

/// Remove any orphan tray icon with our GUID.
///
/// This is called before creating a new tray icon to clean up icons from
/// previous controller instances that were killed without proper cleanup.
/// Using NIF_GUID allows Windows to identify the icon across process restarts.
fn remove_orphan_icon() {
    // Safety: NOTIFYICONDATAW is a POD type, zeroing is valid
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.uFlags = NIF_GUID;
    nid.guidItem = TRAY_ICON_GUID;

    // Try to delete any existing icon with our GUID
    // This will succeed if there's an orphan icon, fail silently otherwise
    let result = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
    if result != 0 {
        info!("Removed orphan tray icon with GUID");
    }
}

/// Spawn the system tray thread.
///
/// Returns a handle to control the tray and sends commands to the provided channel.
pub fn spawn(_command_tx: mpsc::Sender<TrayCommand>) -> Result<TrayHandle> {
    // TODO: Implement tray spawning
    Err(Error::TrayNotSupported)
}
