use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

use crate::model::{InteractionPolicy, WindowMaterial};

#[cfg(target_os = "macos")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
    material: WindowMaterial,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::macos::configure(builder, material)
}

#[cfg(target_os = "linux")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
    _material: WindowMaterial,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::linux::configure(builder)
}

#[cfg(target_os = "windows")]
pub(crate) fn configure<'a, M: Manager<tauri::Wry>>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, M>,
    _material: WindowMaterial,
) -> WebviewWindowBuilder<'a, tauri::Wry, M> {
    crate::windows::configure(builder)
}

#[cfg(target_os = "macos")]
pub(crate) fn is_transparent(material: WindowMaterial) -> bool {
    material != WindowMaterial::Opaque
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn is_transparent(material: WindowMaterial) -> bool {
    material == WindowMaterial::Transparent
}

pub(crate) fn show(window: &WebviewWindow, interaction: InteractionPolicy) -> tauri::Result<()> {
    if interaction == InteractionPolicy::Interactive {
        window.show()?;
        if let Err(error) = window.set_focus() {
            if let Err(hide_error) = window.hide() {
                tracing::warn!(%hide_error, "failed to roll back anchored-window focus failure");
            }
            return Err(error);
        }
        return Ok(());
    }
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
