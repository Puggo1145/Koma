// UNIFIED OS ABSTRACTION BOUNDARY
// Every compile-time platform branch (`#[cfg(target_os = ...)]`) lives strictly inside
// this module so core business modules (`lib`, `window_stage`, `tray`, `library`) remain
// 100% OS-agnostic.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use fallback::*;

pub const CLOSE_WINDOW_MENU_ID: &str = "platform_close_window";

pub struct TrayPlatformSpec {
    pub icon_bytes: &'static [u8],
    pub icon_as_template: bool,
    pub show_menu_on_left_click: bool,
}
