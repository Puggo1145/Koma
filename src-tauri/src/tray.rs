// RUST DEEP MODULE C: NATIVE SYSTEM TRAY & CHASSIS CONTEXT MENU
// Owns both the OS status-bar / taskbar tray icon (monochrome template on macOS,
// crisp full-color Koma icon on Windows) and the wooden-frame right-click context menu.

use crate::library::{clear_library, rescan_library, trigger_dialog_import_from_menu};
use crate::window_stage::{
    apply_placement, ensure_within_monitors, toggle_visibility, PlacementMode, WindowStageState,
};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Manager, WebviewWindow, Wry};

pub struct SharedChassisMenu {
    pub menu: Menu<Wry>,
    pub chk_floating: CheckMenuItem<Wry>,
    pub chk_desktop: CheckMenuItem<Wry>,
    pub chk_normal: CheckMenuItem<Wry>,
}

pub fn install_tray_and_menus(app: &App) -> tauri::Result<()> {
    let handle = app.handle();

    let item_play = MenuItem::with_id(handle, "play_toggle", "播放 / 暂停", true, None::<&str>)?;
    let item_next = MenuItem::with_id(handle, "next_track", "下一首", true, None::<&str>)?;
    let item_prev = MenuItem::with_id(handle, "prev_track", "上一首", true, None::<&str>)?;
    let sep_1 = PredefinedMenuItem::separator(handle)?;

    let item_open_settings = MenuItem::with_id(
        handle,
        "open_settings",
        "曲库设置...",
        true,
        None::<&str>,
    )?;
    let item_import_files = MenuItem::with_id(
        handle,
        "import_files",
        "导入音乐 / 压缩包...",
        true,
        None::<&str>,
    )?;
    let item_import_folder = MenuItem::with_id(
        handle,
        "import_folder",
        "导入文件夹...",
        true,
        None::<&str>,
    )?;
    let item_rescan = MenuItem::with_id(handle, "rescan_library", "刷新曲库", true, None::<&str>)?;
    let item_clear_lib = MenuItem::with_id(handle, "clear_library", "清空曲库", true, None::<&str>)?;
    let sep_2 = PredefinedMenuItem::separator(handle)?;

    let chk_floating = CheckMenuItem::with_id(
        handle,
        "place_floating",
        "置顶把玩模式",
        true,
        true,
        None::<&str>,
    )?;
    let chk_desktop = CheckMenuItem::with_id(
        handle,
        "place_desktop",
        "桌面摆件模式",
        true,
        false,
        None::<&str>,
    )?;
    let chk_normal = CheckMenuItem::with_id(
        handle,
        "place_normal",
        "标准窗口模式",
        true,
        false,
        None::<&str>,
    )?;
    let sep_3 = PredefinedMenuItem::separator(handle)?;

    let item_toggle_vis = MenuItem::with_id(
        handle,
        "toggle_vis",
        "显示 / 隐藏",
        true,
        None::<&str>,
    )?;
    let sep_4 = PredefinedMenuItem::separator(handle)?;
    let item_quit = MenuItem::with_id(handle, "quit_app", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        handle,
        &[
            &item_play,
            &item_next,
            &item_prev,
            &sep_1,
            &item_open_settings,
            &item_import_files,
            &item_import_folder,
            &item_rescan,
            &item_clear_lib,
            &sep_2,
            &chk_floating,
            &chk_desktop,
            &chk_normal,
            &sep_3,
            &item_toggle_vis,
            &sep_4,
            &item_quit,
        ],
    )?;

    app.manage(SharedChassisMenu {
        menu: menu.clone(),
        chk_floating,
        chk_desktop,
        chk_normal,
    });

    #[cfg(target_os = "macos")]
    let (tray_icon, is_template) = (
        Image::from_bytes(include_bytes!("../icons/tray-template.png"))?,
        true,
    );

    #[cfg(not(target_os = "macos"))]
    let (tray_icon, is_template) = (
        Image::from_bytes(include_bytes!("../icons/128x128.png"))?,
        false,
    );

    TrayIconBuilder::with_id("koma-tray")
        .icon(tray_icon)
        .icon_as_template(is_template)
        .tooltip("Koma")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Some(win) = tray.app_handle().get_webview_window("main") {
                    if !win.is_visible().unwrap_or(true) {
                        let _ = win.show();
                    }
                    ensure_within_monitors(&win, true);
                    let is_desktop_mode = win
                        .try_state::<WindowStageState>()
                        .and_then(|s| s.placement.lock().ok().map(|g| *g == PlacementMode::Desktop))
                        .unwrap_or(false);
                    if !is_desktop_mode {
                        let _ = win.set_focus();
                    }
                }
            }
        })
        .build(app)?;

    app.on_menu_event(|app_handle, event| {
        handle_menu_action(app_handle, event.id().as_ref());
    });

    Ok(())
}

#[tauri::command]
pub fn popup_chassis_menu(app: AppHandle, window: WebviewWindow) {
    if let Some(shared) = app.try_state::<SharedChassisMenu>() {
        let _ = window.popup_menu(&shared.menu);
    }
}

fn handle_menu_action(app: &AppHandle, id: &str) {
    let Some(win) = app.get_webview_window("main") else {
        return;
    };

    match id {
        "play_toggle" => {
            let _ = app.emit("tray://play-toggle", ());
        }
        "next_track" => {
            let _ = app.emit("tray://next-track", ());
        }
        "prev_track" => {
            let _ = app.emit("tray://prev-track", ());
        }
        "open_settings" => {
            if !win.is_visible().unwrap_or(true) {
                let _ = win.show();
            }
            let _ = app.emit("tray://open-settings", ());
        }
        "import_files" => {
            trigger_dialog_import_from_menu(app.clone(), "files");
        }
        "import_folder" => {
            trigger_dialog_import_from_menu(app.clone(), "folder");
        }
        "rescan_library" => {
            let _ = rescan_library(app.clone());
        }
        "clear_library" => {
            let _ = clear_library(app.clone());
        }
        "place_floating" => {
            apply_placement(&win, PlacementMode::Floating);
            sync_menu_checkmarks(app, PlacementMode::Floating);
        }
        "place_desktop" => {
            apply_placement(&win, PlacementMode::Desktop);
            sync_menu_checkmarks(app, PlacementMode::Desktop);
        }
        "place_normal" => {
            apply_placement(&win, PlacementMode::Normal);
            sync_menu_checkmarks(app, PlacementMode::Normal);
        }
        "toggle_vis" => {
            toggle_visibility(&win);
        }
        "quit_app" => {
            app.exit(0);
        }
        _ => {}
    }
}

fn sync_menu_checkmarks(app: &AppHandle, mode: PlacementMode) {
    if let Some(shared) = app.try_state::<SharedChassisMenu>() {
        let _ = shared.chk_floating.set_checked(mode == PlacementMode::Floating);
        let _ = shared.chk_desktop.set_checked(mode == PlacementMode::Desktop);
        let _ = shared.chk_normal.set_checked(mode == PlacementMode::Normal);
    }
}
