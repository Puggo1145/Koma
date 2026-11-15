// WINDOWS PLATFORM ADAPTER
// Consolidates Explorer reveal, Win32 case-insensitive path behavior, taskbar tray behavior,
// DWM transparent surface setup, Z-order placement, subclassed drag clamping, and DPI/hotplug recovery.

use super::TrayPlatformSpec;
use crate::window_stage::{
    clamp_window_to_desktop, clamp_window_to_single_monitor, restore_main_window, PlacementMode,
    RectI32,
};
use core::ffi::c_void;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};
use tauri::{AppHandle, CloseRequestApi, Manager, RunEvent, WebviewWindow, Window, Wry};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND,
};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsIconic, SetWindowPos, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST,
    SC_MINIMIZE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, WINDOWPOS,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_EXITSIZEMOVE, WM_MOVING, WM_SETTINGCHANGE, WM_SYSCOMMAND,
    WM_WINDOWPOSCHANGING,
};

pub const CASE_INSENSITIVE_FS: bool = true;

const SUBCLASS_ID: usize = 0x4B4F_4D41; // "KOMA"
static DESKTOP_PINNED: AtomicBool = AtomicBool::new(false);
static SUBCLASS_INSTALLED: AtomicBool = AtomicBool::new(false);

pub fn reveal_in_file_manager(dir: &Path) -> Result<(), String> {
    let raw = dir.to_string_lossy();
    let win_path = raw.strip_prefix(r"\\?\").unwrap_or(&raw).replace('/', "\\");
    Command::new("explorer.exe")
        .arg(win_path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn tray_spec() -> TrayPlatformSpec {
    TrayPlatformSpec {
        icon_bytes: include_bytes!("../../icons/128x128.png"),
        icon_as_template: false,
        show_menu_on_left_click: false,
    }
}

pub fn handle_tray_icon_event(tray: &TrayIcon<Wry>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        if let Some(win) = tray.app_handle().get_webview_window("main") {
            restore_main_window(&win);
        }
    }
}

pub fn configure_app_builder(builder: tauri::Builder<Wry>) -> tauri::Builder<Wry> {
    builder
}

pub fn intercept_close_requested(_window: &Window<Wry>, _api: &CloseRequestApi) -> bool {
    false
}

pub fn handle_run_event(_app_handle: &AppHandle<Wry>, _event: &RunEvent) {}

unsafe extern "system" fn enum_monitor_cb(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    let list = &mut *(data as *mut Vec<RectI32>);
    let mut info: MONITORINFO = core::mem::zeroed();
    info.cbSize = core::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(hmonitor, &mut info) != 0 {
        let w = info.rcWork;
        if w.right > w.left && w.bottom > w.top {
            list.push(RectI32 {
                left: w.left,
                top: w.top,
                right: w.right,
                bottom: w.bottom,
            });
        }
    }
    1
}

unsafe fn collect_monitor_work_areas() -> Vec<RectI32> {
    let mut list: Vec<RectI32> = Vec::with_capacity(4);
    let _ = EnumDisplayMonitors(
        core::ptr::null_mut(),
        core::ptr::null(),
        Some(enum_monitor_cb),
        (&mut list as *mut Vec<RectI32>) as LPARAM,
    );
    list
}

unsafe fn reclamp_hwnd(hwnd: HWND, settle_single_monitor: bool) {
    if IsIconic(hwnd) != 0 {
        return;
    }
    let mut r: RECT = core::mem::zeroed();
    if GetWindowRect(hwnd, &mut r) == 0 {
        return;
    }
    let win_rect = RectI32 {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    };
    let monitors = collect_monitor_work_areas();
    let (nx, ny) = if settle_single_monitor {
        clamp_window_to_single_monitor(win_rect, &monitors)
    } else {
        clamp_window_to_desktop(win_rect, &monitors)
    };
    if (nx, ny) != (r.left, r.top) {
        let _ = SetWindowPos(
            hwnd,
            core::ptr::null_mut(),
            nx,
            ny,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

unsafe extern "system" fn desktop_subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _ref_data: usize,
) -> LRESULT {
    match msg {
        WM_MOVING if lparam != 0 && IsIconic(hwnd) == 0 => {
            let raw = &mut *(lparam as *mut RECT);
            let w = raw.right - raw.left;
            let h = raw.bottom - raw.top;
            let monitors = collect_monitor_work_areas();
            let (nx, ny) = clamp_window_to_desktop(
                RectI32 {
                    left: raw.left,
                    top: raw.top,
                    right: raw.right,
                    bottom: raw.bottom,
                },
                &monitors,
            );
            raw.left = nx;
            raw.top = ny;
            raw.right = nx + w;
            raw.bottom = ny + h;
            return 1;
        }
        WM_WINDOWPOSCHANGING if lparam != 0 => {
            let wp = &mut *(lparam as *mut WINDOWPOS);
            if DESKTOP_PINNED.load(Ordering::Relaxed) && (wp.flags & SWP_NOZORDER) == 0 {
                wp.hwndInsertAfter = HWND_BOTTOM;
            }
            if (wp.flags & SWP_NOMOVE) == 0 && IsIconic(hwnd) == 0 {
                let (w, h) = if (wp.flags & SWP_NOSIZE) == 0 && wp.cx > 0 && wp.cy > 0 {
                    (wp.cx, wp.cy)
                } else {
                    let mut cur: RECT = core::mem::zeroed();
                    if GetWindowRect(hwnd, &mut cur) != 0 {
                        (cur.right - cur.left, cur.bottom - cur.top)
                    } else {
                        (0, 0)
                    }
                };
                if w >= 100 && h >= 100 {
                    let monitors = collect_monitor_work_areas();
                    let (nx, ny) = clamp_window_to_desktop(
                        RectI32 {
                            left: wp.x,
                            top: wp.y,
                            right: wp.x + w,
                            bottom: wp.y + h,
                        },
                        &monitors,
                    );
                    wp.x = nx;
                    wp.y = ny;
                }
            }
        }
        WM_SYSCOMMAND
            if DESKTOP_PINNED.load(Ordering::Relaxed)
                && (wparam & 0xFFF0) == (SC_MINIMIZE as usize) =>
        {
            return 0;
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            let res = DefSubclassProc(hwnd, msg, wparam, lparam);
            reclamp_hwnd(hwnd, true);
            return res;
        }
        WM_EXITSIZEMOVE => {
            let res = DefSubclassProc(hwnd, msg, wparam, lparam);
            reclamp_hwnd(hwnd, false);
            return res;
        }
        _ => {}
    }

    DefSubclassProc(hwnd, msg, wparam, lparam)
}

pub fn reclamp_window(window: &WebviewWindow, settle_single_monitor: bool) {
    let Ok(raw) = window.hwnd() else {
        return;
    };
    unsafe {
        reclamp_hwnd(raw.0 as HWND, settle_single_monitor);
    }
}

pub fn on_window_moved(_window: &WebviewWindow) {
    // Win32 live drag and release clamping are handled natively in desktop_subclass_proc.
}

pub fn configure_window_surface(window: &WebviewWindow) {
    let Ok(raw) = window.hwnd() else {
        return;
    };
    let hwnd: HWND = raw.0 as HWND;

    unsafe {
        let border_none: u32 = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as _,
            &border_none as *const u32 as *const c_void,
            core::mem::size_of::<u32>() as u32,
        );

        let corner_pref: u32 = DWMWCP_DONOTROUND as u32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &corner_pref as *const u32 as *const c_void,
            core::mem::size_of::<u32>() as u32,
        );

        if !SUBCLASS_INSTALLED.swap(true, Ordering::SeqCst) {
            let _ = SetWindowSubclass(hwnd, Some(desktop_subclass_proc), SUBCLASS_ID, 0);
        }
    }
}

pub fn set_window_placement(window: &WebviewWindow, mode: PlacementMode) {
    let Ok(raw) = window.hwnd() else {
        return;
    };
    let hwnd: HWND = raw.0 as HWND;

    match mode {
        PlacementMode::Floating => {
            DESKTOP_PINNED.store(false, Ordering::SeqCst);
            let _ = window.set_skip_taskbar(false);
            let _ = window.set_always_on_top(true);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
        PlacementMode::Desktop => {
            let _ = window.set_always_on_top(false);
            let _ = window.set_skip_taskbar(true);
            DESKTOP_PINNED.store(true, Ordering::SeqCst);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_BOTTOM,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
        PlacementMode::Normal => {
            DESKTOP_PINNED.store(false, Ordering::SeqCst);
            let _ = window.set_skip_taskbar(false);
            let _ = window.set_always_on_top(false);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    HWND_NOTOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                let _ = SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                );
            }
        }
    }
}
