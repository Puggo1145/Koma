mod library;
mod platform;
mod tray;
mod window_stage;

use tauri::Manager;
use window_stage::WindowStageState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    platform::configure_app_builder(tauri::Builder::default())
        .manage(WindowStageState::default())
        .register_asynchronous_uri_scheme_protocol("komapack", library::handle_komapack_protocol)
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
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if platform::intercept_close_requested(window, api) {
                        return;
                    }
                }
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
        .build(tauri::generate_context!())
        .expect("error while building Koma")
        .run(|app_handle, event| platform::handle_run_event(app_handle, &event));
}
