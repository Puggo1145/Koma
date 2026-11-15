// FALLBACK PLATFORM ADAPTER (Linux / other Unix desktops)
// Implements the unified platform contract via portable Tauri APIs and xdg-open.

use super::TrayPlatformSpec;
use crate::window_stage::{
    clamp_window_to_desktop, clamp_window_to_single_monitor, restore_main_window, PlacementMode,
    RectI32,
};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconEvent};
use tauri::{AppHandle, CloseRequestApi, Manager, RunEvent, WebviewWindow, Window, Wry};

pub const CASE_INSENSITIVE_FS: bool = false;

pub fn reveal_in_file_manager(dir: &Path) -> Result<(), String> {
    Command::new("xdg-open")
        .arg(dir)
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

pub fn configure_window_surface(_window: &WebviewWindow) {}

pub fn set_window_placement(window: &WebviewWindow, mode: PlacementMode) {
    let _ = window.set_skip_taskbar(matches!(mode, PlacementMode::Desktop));
    let _ = window.set_always_on_top(matches!(mode, PlacementMode::Floating));
}

pub fn reclamp_window(window: &WebviewWindow, settle_single_monitor: bool) {
    static CLAMPING: AtomicBool = AtomicBool::new(false);
    if CLAMPING.swap(true, Ordering::SeqCst) {
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

    CLAMPING.store(false, Ordering::SeqCst);
}

pub fn on_window_moved(window: &WebviewWindow) {
    reclamp_window(window, false);
}
