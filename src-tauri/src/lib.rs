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
            library::scan_local_yorushika,
            tray::popup_chassis_menu,
        ])
        .setup(|app| {
            if let Some(main_win) = app.get_webview_window("main") {
                window_stage::init_main_window(&main_win);
            }
            tray::install_tray_and_menus(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Yoru RT-01");
}
