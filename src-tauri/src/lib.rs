mod library;
mod tray;
mod window_stage;

use tauri::Manager;
use window_stage::WindowStageState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(WindowStageState::default())
        .invoke_handler(tauri::generate_handler![
            library::load_library,
            library::rescan_library,
            library::import_paths,
            library::pick_and_import,
            library::toggle_library_source,
            library::reimport_library_source,
            library::reveal_library_source,
            library::remove_library_source,
            library::clear_library,
            tray::popup_chassis_menu,
        ])
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let Some(wv) = window.app_handle().get_webview_window("main") {
                    window_stage::handle_window_event(&wv, event);
                }
            }
        })
        .setup(|app| {
            if let Some(main_win) = app.get_webview_window("main") {
                window_stage::init_main_window(&main_win);
            }
            tray::install_tray_and_menus(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Koma");
}
