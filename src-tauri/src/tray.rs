// RUST DEEP MODULE C: MACOS MENU BAR TRAY & CHASSIS CONTEXT MENU
// Owns both the top status-bar monochrome template icon and the wooden-frame right-click menu,
// keeping placement checkmarks (Floating / Desktop / Normal) and Compact Size synchronized.

use crate::library::scan_local_yorushika;
use crate::window_stage::{
    apply_placement, apply_scale, toggle_visibility, ChassisScale, PlacementMode, WindowStageState,
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
    pub chk_compact: CheckMenuItem<Wry>,
}

pub fn install_tray_and_menus(app: &App) -> tauri::Result<()> {
    let handle = app.handle();

    let item_play = MenuItem::with_id(handle, "play_toggle", "播放 / 暂停", true, None::<&str>)?;
    let item_next = MenuItem::with_id(handle, "next_track", "下一首", true, None::<&str>)?;
    let item_prev = MenuItem::with_id(handle, "prev_track", "上一首", true, None::<&str>)?;
    let sep_1 = PredefinedMenuItem::separator(handle)?;

    let chk_floating = CheckMenuItem::with_id(
        handle,
        "place_floating",
        "置顶把玩模式（浮于窗口上方）",
        true,
        true,
        None::<&str>,
    )?;
    let chk_desktop = CheckMenuItem::with_id(
        handle,
        "place_desktop",
        "桌面摆件模式（固定在壁纸层）",
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
    let sep_2 = PredefinedMenuItem::separator(handle)?;

    let chk_compact = CheckMenuItem::with_id(
        handle,
        "toggle_compact",
        "袖珍机身尺寸 (82%)",
        true,
        false,
        None::<&str>,
    )?;
    let item_rescan = MenuItem::with_id(
        handle,
        "rescan_library",
        "重新扫描 ~/Music/Yorushika",
        true,
        None::<&str>,
    )?;
    let item_toggle_vis = MenuItem::with_id(
        handle,
        "toggle_vis",
        "显示 / 隐藏机身",
        true,
        None::<&str>,
    )?;
    let sep_3 = PredefinedMenuItem::separator(handle)?;
    let item_quit = MenuItem::with_id(handle, "quit_app", "退出 Yoru RT-01", true, None::<&str>)?;

    let menu = Menu::with_items(
        handle,
        &[
            &item_play,
            &item_next,
            &item_prev,
            &sep_1,
            &chk_floating,
            &chk_desktop,
            &chk_normal,
            &sep_2,
            &chk_compact,
            &item_rescan,
            &item_toggle_vis,
            &sep_3,
            &item_quit,
        ],
    )?;

    app.manage(SharedChassisMenu {
        menu: menu.clone(),
        chk_floating,
        chk_desktop,
        chk_normal,
        chk_compact,
    });

    let tray_icon = Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;

    TrayIconBuilder::with_id("yoru-tray")
        .icon(tray_icon)
        .icon_as_template(true)
        .tooltip("Yoru RT-01 — ヨルシカ")
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
                    let _ = win.set_focus();
                }
            }
        })
        .build(app)?;

    // Also handle menu events when popped up as a right-click context menu on the window
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
        "toggle_compact" => {
            let next_scale = if let Some(st) = win.try_state::<WindowStageState>() {
                let cur = st.scale.lock().map(|g| *g).unwrap_or(ChassisScale::Standard);
                if cur == ChassisScale::Standard {
                    ChassisScale::Compact
                } else {
                    ChassisScale::Standard
                }
            } else {
                ChassisScale::Standard
            };
            apply_scale(app, &win, next_scale);
            if let Some(shared) = app.try_state::<SharedChassisMenu>() {
                let _ = shared.chk_compact.set_checked(next_scale == ChassisScale::Compact);
            }
        }
        "rescan_library" => {
            let albums = scan_local_yorushika();
            let _ = app.emit("library://updated", albums);
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
