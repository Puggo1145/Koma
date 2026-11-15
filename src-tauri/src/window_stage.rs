// RUST DEEP MODULE A: NATIVE CROSS-PLATFORM WINDOW STAGE CONTROLLER
// Encapsulates frameless transparent window surfaces, DWM border/corner suppression on Windows 11,
// NSWindow clear backing on macOS, Z-order placement transitions (Floating / Desktop Gadget / Normal),
// and DPI-aware multi-monitor desktop edge clamping (live drag + display hotplug/switch recovery).

use serde::Serialize;
use std::sync::Mutex;
use tauri::{Manager, WebviewWindow, WindowEvent};

// Logical window size (tauri.conf.json: 468x484) vs visible .rt01-wood-frame (416x424).
// Transparent drop-shadow padding around the physical wooden chassis:
// top = 16px, left = 26px, right = 26px, bottom = 44px.
const LOGICAL_WIN_W: i32 = 468;
const LOGICAL_WIN_H: i32 = 484;
const LOGICAL_PAD_L: i32 = 26;
const LOGICAL_PAD_R: i32 = 26;
const LOGICAL_PAD_T: i32 = 16;
const LOGICAL_PAD_B: i32 = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectI32 {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectI32 {
    pub const fn width(self) -> i32 {
        self.right - self.left
    }

    pub const fn height(self) -> i32 {
        self.bottom - self.top
    }

    fn intersection_area(self, other: Self) -> i64 {
        let w = (self.right.min(other.right) - self.left.max(other.left)).max(0) as i64;
        let h = (self.bottom.min(other.bottom) - self.top.max(other.top)).max(0) as i64;
        w * h
    }

    fn center_2x(self) -> (i64, i64) {
        (
            self.left as i64 + self.right as i64,
            self.top as i64 + self.bottom as i64,
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct ShadowInsets {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn compute_shadow_insets(win_w: i32, win_h: i32) -> ShadowInsets {
    ShadowInsets {
        left: (win_w * LOGICAL_PAD_L + LOGICAL_WIN_W / 2) / LOGICAL_WIN_W,
        right: (win_w * LOGICAL_PAD_R + LOGICAL_WIN_W / 2) / LOGICAL_WIN_W,
        top: (win_h * LOGICAL_PAD_T + LOGICAL_WIN_H / 2) / LOGICAL_WIN_H,
        bottom: (win_h * LOGICAL_PAD_B + LOGICAL_WIN_H / 2) / LOGICAL_WIN_H,
    }
}

fn push_unique_rect(list: &mut Vec<RectI32>, candidate: RectI32) {
    if candidate.width() <= 0 || candidate.height() <= 0 {
        return;
    }
    // Skip redundant rectangles already fully contained inside an existing safe region
    if list.iter().any(|r| {
        r.left <= candidate.left
            && r.top <= candidate.top
            && r.right >= candidate.right
            && r.bottom >= candidate.bottom
    }) {
        return;
    }
    list.push(candidate);
}

fn expand_adjacent_bridges(base: &[RectI32]) -> Vec<RectI32> {
    const SEAM_TOLERANCE: i32 = 4;
    let mut regions: Vec<RectI32> = Vec::new();
    for &m in base {
        push_unique_rect(&mut regions, m);
    }

    // Two passes synthesize both 2-monitor bridges (horizontal/vertical) and 2x2 quad-wall bridges.
    // Every generated rectangle is strictly a subset of the union of monitor work areas.
    for _ in 0..2 {
        let snapshot = regions.clone();
        for i in 0..snapshot.len() {
            for j in (i + 1)..snapshot.len() {
                let a = snapshot[i];
                let b = snapshot[j];

                // Horizontally touching or overlapping monitors -> shared vertical band bridge
                if a.left.max(b.left) <= a.right.min(b.right) + SEAM_TOLERANCE {
                    let top = a.top.max(b.top);
                    let bottom = a.bottom.min(b.bottom);
                    if bottom > top {
                        push_unique_rect(
                            &mut regions,
                            RectI32 {
                                left: a.left.min(b.left),
                                top,
                                right: a.right.max(b.right),
                                bottom,
                            },
                        );
                    }
                }

                // Vertically touching or overlapping monitors -> shared horizontal band bridge
                if a.top.max(b.top) <= a.bottom.min(b.bottom) + SEAM_TOLERANCE {
                    let left = a.left.max(b.left);
                    let right = a.right.min(b.right);
                    if right > left {
                        push_unique_rect(
                            &mut regions,
                            RectI32 {
                                left,
                                top: a.top.min(b.top),
                                right,
                                bottom: a.bottom.max(b.bottom),
                            },
                        );
                    }
                }
            }
        }
    }

    regions
}

/// Clamps a window rectangle `(x, y, width, height)` so its visible wooden chassis never
/// crosses outside desktop monitor work areas, while allowing seamless dragging across
/// shared boundaries between adjacent monitors.
pub fn clamp_window_to_desktop(win_rect: RectI32, monitors: &[RectI32]) -> (i32, i32) {
    let win_w = win_rect.width();
    let win_h = win_rect.height();
    if monitors.is_empty() || win_w < 100 || win_h < 100 {
        return (win_rect.left, win_rect.top);
    }

    let insets = compute_shadow_insets(win_w, win_h);
    let cw = (win_w - insets.left - insets.right).max(1);
    let ch = (win_h - insets.top - insets.bottom).max(1);
    let cx = win_rect.left + insets.left;
    let cy = win_rect.top + insets.top;

    let regions = expand_adjacent_bridges(monitors);
    let mut best_pos: Option<(i32, i32, i64)> = None;

    for reg in &regions {
        if reg.width() < cw && reg.height() < ch {
            continue;
        }
        let max_x = (reg.right - cw).max(reg.left);
        let max_y = (reg.bottom - ch).max(reg.top);
        let px = cx.clamp(reg.left, max_x);
        let py = cy.clamp(reg.top, max_y);
        let dx = (px - cx) as i64;
        let dy = (py - cy) as i64;
        let dist_sq = dx * dx + dy * dy;

        if best_pos.map_or(true, |(_, _, best_d)| dist_sq < best_d) {
            best_pos = Some((px, py, dist_sq));
            if dist_sq == 0 {
                break;
            }
        }
    }

    if let Some((px, py, _)) = best_pos {
        return (px - insets.left, py - insets.top);
    }

    clamp_window_to_single_monitor(win_rect, monitors)
}

/// Strictly settles the visible wooden chassis inside the single best-matching monitor's
/// work area. Used on startup, monitor hotplug/unplug, primary display switch, and DPI/resolution changes
/// so the player is never stranded off-screen or split across mismatched-DPI displays.
pub fn clamp_window_to_single_monitor(win_rect: RectI32, monitors: &[RectI32]) -> (i32, i32) {
    let win_w = win_rect.width();
    let win_h = win_rect.height();
    if monitors.is_empty() || win_w < 100 || win_h < 100 {
        return (win_rect.left, win_rect.top);
    }

    let insets = compute_shadow_insets(win_w, win_h);
    let cw = (win_w - insets.left - insets.right).max(1);
    let ch = (win_h - insets.top - insets.bottom).max(1);
    let chassis = RectI32 {
        left: win_rect.left + insets.left,
        top: win_rect.top + insets.top,
        right: win_rect.left + insets.left + cw,
        bottom: win_rect.top + insets.top + ch,
    };
    let (ccx, ccy) = chassis.center_2x();

    let target = monitors
        .iter()
        .copied()
        .max_by(|&a, &b| {
            let area_a = chassis.intersection_area(a);
            let area_b = chassis.intersection_area(b);
            if area_a != area_b {
                return area_a.cmp(&area_b);
            }
            let (acx, acy) = a.center_2x();
            let (bcx, bcy) = b.center_2x();
            let dist_a = (acx - ccx).pow(2) + (acy - ccy).pow(2);
            let dist_b = (bcx - ccx).pow(2) + (bcy - ccy).pow(2);
            dist_b.cmp(&dist_a)
        })
        .unwrap_or(monitors[0]);

    let max_x = (target.right - cw).max(target.left);
    let max_y = (target.bottom - ch).max(target.top);
    let px = chassis.left.clamp(target.left, max_x);
    let py = chassis.top.clamp(target.top, max_y);
    (px - insets.left, py - insets.top)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlacementMode {
    Floating,
    Desktop,
    Normal,
}

pub struct WindowStageState {
    pub placement: Mutex<PlacementMode>,
}

impl Default for WindowStageState {
    fn default() -> Self {
        Self {
            placement: Mutex::new(PlacementMode::Floating),
        }
    }
}

pub fn init_main_window(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    macos::configure_transparent_surface(window);

    #[cfg(target_os = "windows")]
    win32::configure_transparent_surface(window);

    apply_placement(window, PlacementMode::Floating);
    ensure_within_monitors(window, true);
}

pub fn apply_placement(window: &WebviewWindow, mode: PlacementMode) {
    if let Some(state) = window.try_state::<WindowStageState>() {
        if let Ok(mut guard) = state.placement.lock() {
            *guard = mode;
        }
    }

    #[cfg(target_os = "macos")]
    macos::set_window_placement(window, mode);

    #[cfg(target_os = "windows")]
    win32::set_window_placement(window, mode);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = window.set_skip_taskbar(matches!(mode, PlacementMode::Desktop));
        let _ = window.set_always_on_top(matches!(mode, PlacementMode::Floating));
    }

    ensure_within_monitors(window, false);
}

pub fn toggle_visibility(window: &WebviewWindow) {
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => {
            let _ = window.show();
            ensure_within_monitors(window, true);
            let is_desktop_mode = window
                .try_state::<WindowStageState>()
                .and_then(|s| s.placement.lock().ok().map(|g| *g == PlacementMode::Desktop))
                .unwrap_or(false);
            if !is_desktop_mode {
                let _ = window.set_focus();
            }
        }
    }
}

pub fn ensure_within_monitors(window: &WebviewWindow, settle_single_monitor: bool) {
    #[cfg(target_os = "windows")]
    {
        win32::reclamp_window(window, settle_single_monitor);
    }

    #[cfg(not(target_os = "windows"))]
    {
        reclamp_tauri_window(window, settle_single_monitor);
    }
}

pub fn handle_window_event(window: &WebviewWindow, event: &WindowEvent) {
    match event {
        WindowEvent::ScaleFactorChanged { .. } => {
            ensure_within_monitors(window, true);
        }
        #[cfg(not(target_os = "windows"))]
        WindowEvent::Moved(_) => {
            reclamp_tauri_window(window, false);
        }
        _ => {}
    }
}

#[cfg(not(target_os = "windows"))]
fn reclamp_tauri_window(window: &WebviewWindow, settle_single_monitor: bool) {
    static CLAMPING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if CLAMPING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }

    if let (Ok(pos), Ok(size), Ok(mons)) = (
        window.outer_position(),
        window.outer_size(),
        window.available_monitors(),
    ) {
        let work_rects: Vec<RectI32> = mons
            .into_iter()
            .map(|m| {
                let wa = m.work_area();
                RectI32 {
                    left: wa.position.x,
                    top: wa.position.y,
                    right: wa.position.x + wa.size.width as i32,
                    bottom: wa.position.y + wa.size.height as i32,
                }
            })
            .collect();

        let win_rect = RectI32 {
            left: pos.x,
            top: pos.y,
            right: pos.x + size.width as i32,
            bottom: pos.y + size.height as i32,
        };

        let (nx, ny) = if settle_single_monitor {
            clamp_window_to_single_monitor(win_rect, &work_rects)
        } else {
            clamp_window_to_desktop(win_rect, &work_rects)
        };

        if (nx, ny) != (pos.x, pos.y) {
            let _ = window.set_position(tauri::PhysicalPosition::new(nx, ny));
        }
    }

    CLAMPING.store(false, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(target_os = "windows")]
mod win32 {
    use super::{
        clamp_window_to_desktop, clamp_window_to_single_monitor, PlacementMode, RectI32,
    };
    use core::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::WebviewWindow;
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, IsIconic, SetWindowPos, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOP,
        HWND_TOPMOST, SC_MINIMIZE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SWP_SHOWWINDOW, WINDOWPOS, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_EXITSIZEMOVE, WM_MOVING,
        WM_SETTINGCHANGE, WM_SYSCOMMAND, WM_WINDOWPOSCHANGING,
    };

    const SUBCLASS_ID: usize = 0x4B4F_4D41; // "KOMA"
    static DESKTOP_PINNED: AtomicBool = AtomicBool::new(false);
    static SUBCLASS_INSTALLED: AtomicBool = AtomicBool::new(false);

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

    pub fn configure_transparent_surface(window: &WebviewWindow) {
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
}

#[cfg(target_os = "macos")]
mod macos {
    use super::PlacementMode;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use tauri::WebviewWindow;

    const K_CG_NORMAL_WINDOW_LEVEL_KEY: i32 = 4;
    const K_CG_FLOATING_WINDOW_LEVEL_KEY: i32 = 5;
    const K_CG_DESKTOP_ICON_WINDOW_LEVEL_KEY: i32 = 18;

    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const STATIONARY: usize = 1 << 4;
    const IGNORES_CYCLE: usize = 1 << 6;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;

    extern "C" {
        fn CGWindowLevelForKey(key: i32) -> i32;
    }

    pub fn configure_transparent_surface(window: &WebviewWindow) {
        let Ok(raw_ptr) = window.ns_window() else {
            return;
        };
        unsafe {
            let ns_win = &*(raw_ptr as *const AnyObject);
            let _: () = msg_send![ns_win, setOpaque: false];
            let _: () = msg_send![ns_win, setHasShadow: false];
            let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
            let _: () = msg_send![ns_win, setBackgroundColor: clear];
        }
    }

    pub fn set_window_placement(window: &WebviewWindow, mode: PlacementMode) {
        let Ok(raw_ptr) = window.ns_window() else {
            return;
        };
        unsafe {
            let ns_win = &*(raw_ptr as *const AnyObject);
            let (level, behavior): (isize, usize) = match mode {
                PlacementMode::Floating => (
                    CGWindowLevelForKey(K_CG_FLOATING_WINDOW_LEVEL_KEY) as isize,
                    CAN_JOIN_ALL_SPACES | FULL_SCREEN_AUXILIARY,
                ),
                PlacementMode::Desktop => (
                    (CGWindowLevelForKey(K_CG_DESKTOP_ICON_WINDOW_LEVEL_KEY) + 1) as isize,
                    CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE,
                ),
                PlacementMode::Normal => (
                    CGWindowLevelForKey(K_CG_NORMAL_WINDOW_LEVEL_KEY) as isize,
                    0,
                ),
            };
            let _: () = msg_send![ns_win, setLevel: level];
            let _: () = msg_send![ns_win, setCollectionBehavior: behavior];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_win(x: i32, y: i32) -> RectI32 {
        RectI32 {
            left: x,
            top: y,
            right: x + LOGICAL_WIN_W,
            bottom: y + LOGICAL_WIN_H,
        }
    }

    #[test]
    fn clamps_visible_wood_chassis_flush_to_single_monitor_edges() {
        let mon = [RectI32 {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032, // 48px taskbar
        }];

        // Top-left overflow -> wood frame stops at (0, 0), so window top-left = (-26, -16)
        assert_eq!(clamp_window_to_desktop(make_win(-200, -150), &mon), (-26, -16));

        // Bottom-right overflow -> wood frame (416x424) stops at (1920-416=1504, 1032-424=608),
        // so window top-left = (1504 - 26 = 1478, 608 - 16 = 592)
        assert_eq!(clamp_window_to_desktop(make_win(1800, 900), &mon), (1478, 592));
    }

    #[test]
    fn allows_seamless_drag_across_adjacent_monitors_while_blocking_outer_edges() {
        let monitors = [
            RectI32 {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1032,
            },
            RectI32 {
                left: 1920,
                top: 0,
                right: 3840,
                bottom: 1032,
            },
        ];

        // Straddling the seam at x = 1920 is 100% allowed without snagging
        let straddling = make_win(1740, 220);
        assert_eq!(clamp_window_to_desktop(straddling, &monitors), (1740, 220));

        // Outer right edge of 2nd monitor clamps at 3840 - 416 - 26 = 3398
        assert_eq!(
            clamp_window_to_desktop(make_win(3650, 220), &monitors),
            (3398, 220)
        );
    }

    #[test]
    fn recovers_offscreen_window_when_secondary_monitor_disconnects() {
        let remaining_primary = [RectI32 {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        }];

        // Window was previously on unplugged right-hand monitor at x = 2600
        let stranded = make_win(2600, 300);
        assert_eq!(
            clamp_window_to_single_monitor(stranded, &remaining_primary),
            (1478, 300)
        );

        // Window was previously on unplugged left-hand negative-coord monitor at x = -1400
        let stranded_left = make_win(-1400, -40);
        assert_eq!(
            clamp_window_to_single_monitor(stranded_left, &remaining_primary),
            (-26, -16)
        );
    }
}
