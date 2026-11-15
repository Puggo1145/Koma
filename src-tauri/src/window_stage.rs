// RUST DEEP MODULE A: MACOS NATIVE WINDOW STAGE CONTROLLER
// Encapsulates frameless transparent NSWindow backing, CoreGraphics window-level transitions
// (Floating / Desktop Wallpaper Layer / Normal), Space collection behaviors, and compact scaling.

use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, Size, WebviewWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlacementMode {
    Floating,
    Desktop,
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChassisScale {
    Standard,
    Compact,
}

pub struct WindowStageState {
    pub placement: Mutex<PlacementMode>,
    pub scale: Mutex<ChassisScale>,
}

impl Default for WindowStageState {
    fn default() -> Self {
        Self {
            placement: Mutex::new(PlacementMode::Floating),
            scale: Mutex::new(ChassisScale::Standard),
        }
    }
}

pub fn init_main_window(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    macos::configure_transparent_surface(window);

    apply_placement(window, PlacementMode::Floating);
}

pub fn apply_placement(window: &WebviewWindow, mode: PlacementMode) {
    if let Some(state) = window.try_state::<WindowStageState>() {
        if let Ok(mut guard) = state.placement.lock() {
            *guard = mode;
        }
    }

    #[cfg(target_os = "macos")]
    macos::set_window_placement(window, mode);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_always_on_top(matches!(mode, PlacementMode::Floating));
    }
}

pub fn apply_scale(app: &AppHandle, window: &WebviewWindow, scale: ChassisScale) {
    if let Some(state) = window.try_state::<WindowStageState>() {
        if let Ok(mut guard) = state.scale.lock() {
            *guard = scale;
        }
    }

    let (w, h) = match scale {
        ChassisScale::Standard => (468.0, 484.0),
        ChassisScale::Compact => (396.0, 410.0),
    };
    let _ = window.set_size(Size::Logical(LogicalSize::new(w, h)));
    let _ = app.emit("stage://scale-changed", scale);
}

pub fn toggle_visibility(window: &WebviewWindow) {
    match window.is_visible() {
        Ok(true) => {
            let _ = window.hide();
        }
        _ => {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::PlacementMode;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use tauri::WebviewWindow;

    // CoreGraphics CGWindowLevelKey constants from MacOSX.sdk/CGWindowLevel.h
    const K_CG_NORMAL_WINDOW_LEVEL_KEY: i32 = 4;
    const K_CG_FLOATING_WINDOW_LEVEL_KEY: i32 = 5;
    const K_CG_DESKTOP_ICON_WINDOW_LEVEL_KEY: i32 = 18;

    // NSWindowCollectionBehavior bitflags
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
