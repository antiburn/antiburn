use crate::companion::CompanionWindow;
#[cfg(not(target_os = "macos"))]
use tauri::{Manager, WebviewWindowBuilder};

#[cfg(target_os = "linux")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::linux::configure(builder)
}

#[cfg(target_os = "windows")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::windows::configure(builder)
}

pub(crate) fn show(window: &CompanionWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    return window.show();
    #[cfg(target_os = "linux")]
    return crate::linux::show_without_activation(window);
    #[cfg(target_os = "windows")]
    return crate::windows::show_without_activation(window);
}

pub(crate) fn hide(window: &CompanionWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    return window.hide();
    #[cfg(target_os = "linux")]
    return crate::linux::hide(window);
    #[cfg(target_os = "windows")]
    return crate::windows::hide(window);
}
