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
pub(crate) const LOGICAL_WIN_W: i32 = 468;
pub(crate) const LOGICAL_WIN_H: i32 = 484;
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
    crate::platform::configure_window_surface(window);
    apply_placement(window, PlacementMode::Floating);
    ensure_within_monitors(window, true);
}

pub fn apply_placement(window: &WebviewWindow, mode: PlacementMode) {
    if let Some(state) = window.try_state::<WindowStageState>() {
        if let Ok(mut guard) = state.placement.lock() {
            *guard = mode;
        }
    }

    crate::platform::set_window_placement(window, mode);
    ensure_within_monitors(window, false);
}

pub fn restore_main_window(window: &WebviewWindow) {
    if window.is_minimized().unwrap_or(false) {
        let _ = window.unminimize();
    }
    if !window.is_visible().unwrap_or(true) {
        let _ = window.show();
    }
    let is_desktop_mode = window
        .try_state::<WindowStageState>()
        .and_then(|s| s.placement.lock().ok().map(|g| *g == PlacementMode::Desktop))
        .unwrap_or(false);
    if is_desktop_mode {
        crate::platform::set_window_placement(window, PlacementMode::Desktop);
        ensure_within_monitors(window, true);
    } else {
        ensure_within_monitors(window, true);
        let _ = window.set_focus();
    }
}

pub fn toggle_visibility(window: &WebviewWindow) {
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => {
            restore_main_window(window);
        }
    }
}

pub fn ensure_within_monitors(window: &WebviewWindow, settle_single_monitor: bool) {
    crate::platform::reclamp_window(window, settle_single_monitor);
}

pub fn handle_window_event(window: &WebviewWindow, event: &WindowEvent) {
    match event {
        WindowEvent::ScaleFactorChanged { .. } => {
            ensure_within_monitors(window, true);
        }
        WindowEvent::Moved(_) => {
            crate::platform::on_window_moved(window);
        }
        _ => {}
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
