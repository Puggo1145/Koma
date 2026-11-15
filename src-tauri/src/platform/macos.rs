// macOS PLATFORM ADAPTER
// Consolidates Finder reveal, case-insensitive/APFS path key behavior, menu-bar status item spec,
// frameless AppKit menu (without Enter Full Screen, Cmd+W hide-window), Dock Reopen,
// transparent NSWindow clear backing, Cocoa visibleFrame mixed-DPI clamping, post-drag settle,
// and display/Dock reconfiguration notifications.

use super::{TrayPlatformSpec, CLOSE_WINDOW_MENU_ID};
use crate::window_stage::{
    clamp_window_to_desktop, clamp_window_to_single_monitor, restore_main_window, PlacementMode,
    RectI32,
};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSColor, NSEvent, NSScreen, NSWindow,
    NSWindowCollectionBehavior,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSPoint};
use std::path::Path;
use std::process::Command;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, WINDOW_SUBMENU_ID};
use tauri::tray::{TrayIcon, TrayIconEvent};
use tauri::{AppHandle, CloseRequestApi, Manager, RunEvent, WebviewWindow, Window, Wry};

pub const CASE_INSENSITIVE_FS: bool = true;

const K_CG_NORMAL_WINDOW_LEVEL_KEY: i32 = 4;
const K_CG_FLOATING_WINDOW_LEVEL_KEY: i32 = 5;
const K_CG_DESKTOP_ICON_WINDOW_LEVEL_KEY: i32 = 18;

static CLAMPING: AtomicBool = AtomicBool::new(false);
static DRAG_SETTLE_PENDING: AtomicBool = AtomicBool::new(false);
static SCREEN_OBSERVER_INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" {
    fn CGWindowLevelForKey(key: i32) -> i32;
}

pub fn reveal_in_file_manager(dir: &Path) -> Result<(), String> {
    Command::new("open")
        .arg(dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn tray_spec() -> TrayPlatformSpec {
    TrayPlatformSpec {
        icon_bytes: include_bytes!("../../icons/tray-template.png"),
        icon_as_template: true,
        show_menu_on_left_click: true,
    }
}

pub fn handle_tray_icon_event(_tray: &TrayIcon<Wry>, _event: TrayIconEvent) {
    // Left-clicking the macOS menu bar icon opens the status menu natively via show_menu_on_left_click.
}

pub fn configure_app_builder(builder: tauri::Builder<Wry>) -> tauri::Builder<Wry> {
    builder.menu(build_macos_app_menu)
}

pub fn intercept_close_requested(window: &Window<Wry>, api: &CloseRequestApi) -> bool {
    api.prevent_close();
    let _ = window.hide();
    true
}

pub fn handle_run_event(app_handle: &AppHandle<Wry>, event: &RunEvent) {
    if let RunEvent::Reopen { .. } = event {
        if let Some(win) = app_handle.get_webview_window("main") {
            restore_main_window(&win);
        }
    }
}

fn build_macos_app_menu(app_handle: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let pkg_info = app_handle.package_info();
    let config = app_handle.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    let app_submenu = Submenu::with_items(
        app_handle,
        pkg_info.name.clone(),
        true,
        &[
            &PredefinedMenuItem::about(app_handle, None, Some(about_metadata))?,
            &PredefinedMenuItem::separator(app_handle)?,
            &PredefinedMenuItem::services(app_handle, None)?,
            &PredefinedMenuItem::separator(app_handle)?,
            &PredefinedMenuItem::hide(app_handle, None)?,
            &PredefinedMenuItem::hide_others(app_handle, None)?,
            &PredefinedMenuItem::show_all(app_handle, None)?,
            &PredefinedMenuItem::separator(app_handle)?,
            &PredefinedMenuItem::quit(app_handle, None)?,
        ],
    )?;

    let edit_submenu = Submenu::with_items(
        app_handle,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app_handle, None)?,
            &PredefinedMenuItem::redo(app_handle, None)?,
            &PredefinedMenuItem::separator(app_handle)?,
            &PredefinedMenuItem::cut(app_handle, None)?,
            &PredefinedMenuItem::copy(app_handle, None)?,
            &PredefinedMenuItem::paste(app_handle, None)?,
            &PredefinedMenuItem::select_all(app_handle, None)?,
        ],
    )?;

    let hide_window_item = MenuItem::with_id(
        app_handle,
        CLOSE_WINDOW_MENU_ID,
        "关闭窗口",
        true,
        Some("CmdOrCtrl+W"),
    )?;
    let window_submenu = Submenu::with_id_and_items(
        app_handle,
        WINDOW_SUBMENU_ID,
        "窗口",
        true,
        &[
            &PredefinedMenuItem::minimize(app_handle, None)?,
            &PredefinedMenuItem::separator(app_handle)?,
            &hide_window_item,
        ],
    )?;

    Menu::with_items(app_handle, &[&app_submenu, &edit_submenu, &window_submenu])
}

pub fn cocoa_rect_to_top_down(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    primary_screen_height: f64,
) -> RectI32 {
    let left = x.round() as i32;
    let right = (x + width).round() as i32;
    let top = (primary_screen_height - (y + height)).round() as i32;
    let bottom = (primary_screen_height - y).round() as i32;
    RectI32 {
        left,
        top,
        right,
        bottom,
    }
}

pub fn top_down_origin_to_cocoa(
    nx: i32,
    ny: i32,
    win_height: f64,
    primary_screen_height: f64,
) -> (f64, f64) {
    (nx as f64, primary_screen_height - (ny as f64) - win_height)
}

struct ClampingGuard;

impl ClampingGuard {
    fn try_enter() -> Option<Self> {
        if CLAMPING.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(Self)
        }
    }
}

impl Drop for ClampingGuard {
    fn drop(&mut self) {
        CLAMPING.store(false, Ordering::SeqCst);
    }
}

fn with_main_thread(
    window: &WebviewWindow,
    f: impl FnOnce(MainThreadMarker, WebviewWindow) + Send + 'static,
) {
    if let Some(mtm) = MainThreadMarker::new() {
        f(mtm, window.clone());
    } else {
        let w = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                f(mtm, w);
            }
        });
    }
}

pub fn configure_window_surface(window: &WebviewWindow) {
    with_main_thread(window, |_mtm, win| {
        if let Ok(raw_ptr) = win.ns_window() {
            let ns_win = unsafe { &*(raw_ptr as *const NSWindow) };
            ns_win.setOpaque(false);
            ns_win.setHasShadow(false);
            ns_win.setBackgroundColor(Some(&NSColor::clearColor()));
        }

        if !SCREEN_OBSERVER_INSTALLED.swap(true, Ordering::SeqCst) {
            let win_for_observer = win.clone();
            let block = RcBlock::new(move |_notif: NonNull<NSNotification>| {
                reclamp_window(&win_for_observer, true);
            });
            let observer = unsafe {
                NSNotificationCenter::defaultCenter()
                    .addObserverForName_object_queue_usingBlock(
                        Some(NSApplicationDidChangeScreenParametersNotification),
                        None,
                        None,
                        &block,
                    )
            };
            std::mem::forget(observer);
        }
    });
}

pub fn set_window_placement(window: &WebviewWindow, mode: PlacementMode) {
    with_main_thread(window, move |_mtm, win| {
        let Ok(raw_ptr) = win.ns_window() else {
            return;
        };
        let ns_win = unsafe { &*(raw_ptr as *const NSWindow) };
        match mode {
            PlacementMode::Floating => {
                let _ = win.set_minimizable(true);
                ns_win.setLevel(unsafe { CGWindowLevelForKey(K_CG_FLOATING_WINDOW_LEVEL_KEY) }
                    as isize);
                ns_win.setCollectionBehavior(
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::FullScreenAuxiliary,
                );
                if win.is_visible().unwrap_or(false) {
                    ns_win.orderFrontRegardless();
                }
            }
            PlacementMode::Desktop => {
                if win.is_minimized().unwrap_or(false) {
                    let _ = win.unminimize();
                }
                let _ = win.set_minimizable(false);
                ns_win.setLevel(
                    (unsafe { CGWindowLevelForKey(K_CG_DESKTOP_ICON_WINDOW_LEVEL_KEY) } + 1)
                        as isize,
                );
                ns_win.setCollectionBehavior(
                    NSWindowCollectionBehavior::CanJoinAllSpaces
                        | NSWindowCollectionBehavior::Stationary
                        | NSWindowCollectionBehavior::IgnoresCycle
                        | NSWindowCollectionBehavior::FullScreenNone,
                );
                ns_win.orderBack(None);
            }
            PlacementMode::Normal => {
                let _ = win.set_minimizable(true);
                ns_win.setLevel(
                    unsafe { CGWindowLevelForKey(K_CG_NORMAL_WINDOW_LEVEL_KEY) } as isize,
                );
                ns_win.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenNone);
                if win.is_visible().unwrap_or(false) {
                    ns_win.makeKeyAndOrderFront(None);
                    let _ = win.set_focus();
                }
            }
        }
    });
}

fn reclamp_window_on_main(
    mtm: MainThreadMarker,
    win: &WebviewWindow,
    settle_single_monitor: bool,
) {
    let Some(_guard) = ClampingGuard::try_enter() else {
        return;
    };
    let Ok(raw_ptr) = win.ns_window() else {
        return;
    };
    let ns_win = unsafe { &*(raw_ptr as *const NSWindow) };
    if ns_win.isMiniaturized() {
        return;
    }

    let screens = NSScreen::screens(mtm);
    let count = screens.count();
    if count == 0 {
        return;
    }
    let primary_h = screens.objectAtIndex(0).frame().size.height;
    let mut work_rects: Vec<RectI32> = Vec::with_capacity(count);
    for i in 0..count {
        let vf = screens.objectAtIndex(i).visibleFrame();
        let rect = cocoa_rect_to_top_down(
            vf.origin.x,
            vf.origin.y,
            vf.size.width,
            vf.size.height,
            primary_h,
        );
        if rect.width() > 0 && rect.height() > 0 {
            work_rects.push(rect);
        }
    }
    if work_rects.is_empty() {
        return;
    }

    let wf = ns_win.frame();
    let win_rect = cocoa_rect_to_top_down(
        wf.origin.x,
        wf.origin.y,
        wf.size.width,
        wf.size.height,
        primary_h,
    );

    let (nx, ny) = if settle_single_monitor {
        clamp_window_to_single_monitor(win_rect, &work_rects)
    } else {
        clamp_window_to_desktop(win_rect, &work_rects)
    };

    if (nx, ny) != (win_rect.left, win_rect.top) {
        let (cx, cy) = top_down_origin_to_cocoa(nx, ny, wf.size.height, primary_h);
        ns_win.setFrameOrigin(NSPoint::new(cx, cy));
    }
}

pub fn reclamp_window(window: &WebviewWindow, settle_single_monitor: bool) {
    with_main_thread(window, move |mtm, win| {
        reclamp_window_on_main(mtm, &win, settle_single_monitor);
    });
}

pub fn on_window_moved(window: &WebviewWindow) {
    if CLAMPING.load(Ordering::SeqCst) {
        return;
    }
    with_main_thread(window, move |mtm, win| {
        let left_pressed = (NSEvent::pressedMouseButtons() & 1) != 0;
        if !left_pressed {
            reclamp_window_on_main(mtm, &win, false);
            return;
        }
        if DRAG_SETTLE_PENDING.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::spawn(move || {
            for _ in 0..2000 {
                std::thread::sleep(Duration::from_millis(30));
                let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
                let w_check = win.clone();
                if win
                    .run_on_main_thread(move || {
                        let pressed = (NSEvent::pressedMouseButtons() & 1) != 0;
                        if !pressed {
                            DRAG_SETTLE_PENDING.store(false, Ordering::SeqCst);
                            if let Some(mtm) = MainThreadMarker::new() {
                                reclamp_window_on_main(mtm, &w_check, false);
                            }
                        }
                        let _ = tx.send(pressed);
                    })
                    .is_err()
                {
                    DRAG_SETTLE_PENDING.store(false, Ordering::SeqCst);
                    return;
                }
                if !rx.recv().unwrap_or(false) {
                    return;
                }
            }
            DRAG_SETTLE_PENDING.store(false, Ordering::SeqCst);
            reclamp_window(&win, false);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window_stage::{LOGICAL_WIN_H, LOGICAL_WIN_W};

    fn make_win(x: i32, y: i32) -> RectI32 {
        RectI32 {
            left: x,
            top: y,
            right: x + LOGICAL_WIN_W,
            bottom: y + LOGICAL_WIN_H,
        }
    }

    #[test]
    fn converts_macos_cocoa_visible_frames_across_mixed_dpi_monitors() {
        let primary_h = 982.0;
        let primary_visible = cocoa_rect_to_top_down(0.0, 70.0, 1512.0, 874.0, primary_h);
        assert_eq!(
            primary_visible,
            RectI32 {
                left: 0,
                top: 38,
                right: 1512,
                bottom: 912,
            }
        );

        let external_visible = cocoa_rect_to_top_down(1512.0, 0.0, 1920.0, 1055.0, primary_h);
        assert_eq!(
            external_visible,
            RectI32 {
                left: 1512,
                top: -73,
                right: 3432,
                bottom: 982,
            }
        );

        let monitors = [primary_visible, external_visible];

        let straddling = make_win(1320, 120);
        assert_eq!(clamp_window_to_desktop(straddling, &monitors), (1320, 120));

        let above_menubar = make_win(300, -40);
        assert_eq!(clamp_window_to_desktop(above_menubar, &monitors), (300, 22));

        let inside_dock = make_win(300, 700);
        assert_eq!(clamp_window_to_desktop(inside_dock, &monitors), (300, 472));

        let cocoa_origin = top_down_origin_to_cocoa(300, 22, LOGICAL_WIN_H as f64, primary_h);
        assert_eq!(cocoa_origin, (300.0, primary_h - 22.0 - 484.0));
    }
}
