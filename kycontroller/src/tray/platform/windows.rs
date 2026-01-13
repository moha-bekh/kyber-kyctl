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

use log::{error, info, warn};
use muda::{ContextMenu, Menu, MenuEvent, MenuId, MenuItem};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::thread;
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

/// Context for the tray window procedure to access the menu.
struct TrayContext {
    menu: Menu,
}

/// Window procedure for the hidden tray window.
///
/// Handles WM_TRAYICON messages to show the context menu using muda's show_context_menu_for_hwnd.
/// Context is stored in window user data via WM_NCCREATE/GetWindowLongPtrW pattern.
unsafe extern "system" fn tray_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Store context pointer in window user data on creation
    if msg == WM_NCCREATE {
        let create_struct = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create_struct.lpCreateParams as isize);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }

    // Retrieve context from window user data
    let userdata = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if userdata == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let ctx = &*(userdata as *const TrayContext);

    if msg == WM_TRAYICON {
        let event = (lparam & 0xFFFF) as u32;
        if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
            // Required to make menu dismiss when clicking elsewhere
            SetForegroundWindow(hwnd);
            // Use muda's show_context_menu_for_hwnd - pass None to use cursor position
            ctx.menu.show_context_menu_for_hwnd(hwnd as isize, None);
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
    /// The `ctx` pointer is stored in the window's user data for the window procedure.
    /// The caller must ensure `ctx` remains valid for the lifetime of the tray icon.
    ///
    /// This also attempts to remove any orphan icon with the same GUID first.
    fn new(icon_data: &[u8], tooltip: &str, ctx: *const TrayContext) -> Result<Self> {
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

        // Create hidden window for receiving messages.
        // Pass context pointer via lpParam for WM_NCCREATE to store in user data.
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
                ctx as *const std::ffi::c_void,
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

/// Build the context menu for the tray icon.
fn build_menu() -> Result<Menu> {
    let menu = Menu::new();

    let stop_item = MenuItem::with_id(MENU_STOP_ID, "Stop Kyber", true, None);
    let restart_item = MenuItem::with_id(MENU_RESTART_ID, "Restart Kyber", true, None);

    menu.append(&restart_item).map_err(|e| Error::TrayInit {
        reason: e.to_string(),
    })?;
    menu.append(&stop_item).map_err(|e| Error::TrayInit {
        reason: e.to_string(),
    })?;

    Ok(menu)
}

/// Wait for events using MsgWaitForMultipleObjectsEx.
///
/// This is more efficient than polling because the thread sleeps until:
/// - The shutdown event is signaled
/// - The wake event is signaled (status update available)
/// - Windows messages arrive (for tray icon interactions)
///
/// # Arguments
///
/// * `shutdown_event` - Windows event handle for shutdown signaling
/// * `wake_event` - Windows event handle for waking to check status updates
///
/// # Safety
///
/// This function uses unsafe Windows API calls. Safety is ensured by:
/// - Both events are valid handles created by CreateEventW
/// - All parameters to MsgWaitForMultipleObjectsEx are valid
fn wait_for_events(shutdown_event: SendHandle, wake_event: SendHandle) -> WaitResult {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::INFINITE;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, QS_ALLINPUT,
    };

    let handles = [shutdown_event.as_raw(), wake_event.as_raw()];

    // Safety: both handles are valid, other parameters are constants
    let result = unsafe {
        MsgWaitForMultipleObjectsEx(
            2,                   // nCount - number of handles
            handles.as_ptr(),    // pHandles - array of handles
            INFINITE,            // dwMilliseconds - wait indefinitely
            QS_ALLINPUT,         // dwWakeMask - wake on any input
            MWMO_INPUTAVAILABLE, // dwFlags - return when input is available
        )
    };

    match result {
        WAIT_OBJECT_0 => WaitResult::Shutdown,
        x if x == WAIT_OBJECT_0 + 1 => WaitResult::Wake,
        x if x == WAIT_OBJECT_0 + 2 => WaitResult::WindowsMessage,
        _ => WaitResult::Shutdown, // Error case - treat as shutdown
    }
}

/// Process all pending Windows messages.
///
/// # Safety
///
/// This function uses unsafe Windows API calls. Safety is ensured by:
/// - Proper initialization of MSG struct with zeroed memory
/// - Valid pointer passed to PeekMessageW
/// - Standard Windows message dispatch pattern (TranslateMessage + DispatchMessageW)
fn pump_windows_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    // Safety: MSG is a POD type, zeroing is valid initialization
    let mut msg: MSG = unsafe { std::mem::zeroed() };

    // Safety: PeekMessageW with null hwnd processes all thread messages
    // PM_REMOVE removes messages from queue after processing
    while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
        // Safety: Standard Windows message processing pattern
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Handle a menu item click event.
fn handle_menu_event(id: &MenuId, command_tx: &mpsc::Sender<TrayCommand>) {
    match id.as_ref() {
        MENU_STOP_ID => {
            info!("Tray: Stop requested");
            let _ = command_tx.blocking_send(TrayCommand::Stop);
        }
        MENU_RESTART_ID => {
            info!("Tray: Restart requested");
            let _ = command_tx.blocking_send(TrayCommand::Restart);
        }
        _ => {}
    }
}

/// Update the tray icon tooltip based on status.
fn update_tooltip(tray: &NativeTrayIcon, status: &TrayStatus) {
    let tooltip = status.tooltip();
    info!("Tray status: {tooltip}");
    tray.set_tooltip(&tooltip);
}

/// Main tray event loop (runs on dedicated thread).
///
/// This function must run on a dedicated OS thread because:
/// 1. Windows tray icons require a message pump on the creating thread
/// 2. The message pump is inherently blocking
/// 3. muda menu library requires same-thread access
///
/// Communication with async code uses:
/// - Windows event (`shutdown_event`) for shutdown signaling
/// - Windows event (`wake_event`) for waking to check status updates
/// - tokio::sync::mpsc channel for status updates
fn run_tray_loop(
    mut status_rx: mpsc::Receiver<TrayStatus>,
    command_tx: mpsc::Sender<TrayCommand>,
    shutdown_event: SendHandle,
    wake_event: SendHandle,
) -> Result<()> {
    info!("Tray thread started");

    // Build the context menu first (needed before creating the tray icon)
    let menu = match build_menu() {
        Ok(m) => m,
        Err(e) => {
            error!("Failed to build tray menu: {e}");
            return Err(e);
        }
    };

    // Create context for the window procedure (stored in window user data)
    let tray_ctx = TrayContext { menu };

    // Retry creating tray icon - at boot time the desktop may not be ready yet.
    // When running as a service, controller starts before user login, so we need
    // to wait for explorer.exe to start (can take several minutes).
    const MAX_RETRIES: u32 = 150; // 5 minutes total (150 x 2s)
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

    let mut retry_count = 0u32;
    let tray_icon = loop {
        match NativeTrayIcon::new(ICON_BYTES, "Kyber - Starting...", &tray_ctx) {
            Ok(tray) => break tray,
            Err(e) => {
                retry_count += 1;
                if retry_count > MAX_RETRIES {
                    return Err(Error::TrayInit {
                        reason: format!("Failed after {} retries: {e}", MAX_RETRIES),
                    });
                }
                warn!(
                    "Tray icon creation failed (attempt {}/{}): {e}. Retrying in {:?}...",
                    retry_count, MAX_RETRIES, RETRY_DELAY
                );
                thread::sleep(RETRY_DELAY);
            }
        }
    };

    info!("System tray icon created (using NIF_GUID for orphan cleanup)");

    let menu_event_rx = MenuEvent::receiver();

    // Main event loop using MsgWaitForMultipleObjectsEx
    loop {
        match wait_for_events(shutdown_event, wake_event) {
            WaitResult::Shutdown => {
                info!("Tray shutdown signal received");
                break;
            }
            WaitResult::Wake => {
                // Wake event signaled - check for status updates
                while let Ok(status) = status_rx.try_recv() {
                    update_tooltip(&tray_icon, &status);
                }
            }
            WaitResult::WindowsMessage => {
                // Process all pending Windows messages
                pump_windows_messages();

                // Check for menu events after processing messages
                if let Ok(event) = menu_event_rx.try_recv() {
                    handle_menu_event(&event.id, &command_tx);
                }
            }
        }
    }

    info!("Removing tray icon");
    // tray_icon is dropped here, which calls Shell_NotifyIconW(NIM_DELETE) with NIF_GUID
    Ok(())
}

/// Spawn the system tray thread.
///
/// Returns a handle to control the tray and sends commands to the provided channel.
pub fn spawn(_command_tx: mpsc::Sender<TrayCommand>) -> Result<TrayHandle> {
    // TODO: Implement tray spawning
    Err(Error::TrayNotSupported)
}
