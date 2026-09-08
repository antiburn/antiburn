use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

#[cfg(target_os = "macos")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
    corner_radius: f64,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::macos::configure(builder, corner_radius)
}

#[cfg(target_os = "linux")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
    _corner_radius: f64,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::linux::configure(builder)
}

#[cfg(target_os = "windows")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
    _corner_radius: f64,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::windows::configure(builder)
}

pub(crate) fn show(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    return crate::macos::show_without_activation(window);
    #[cfg(target_os = "linux")]
    return crate::linux::show_without_activation(window);
    #[cfg(target_os = "windows")]
    return crate::windows::show_without_activation(window);
}

pub(crate) fn hide(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    return crate::macos::hide(window);
    #[cfg(target_os = "linux")]
    return crate::linux::hide(window);
    #[cfg(target_os = "windows")]
    return crate::windows::hide(window);
}
