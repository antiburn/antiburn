#[cfg(target_os = "macos")]
pub(crate) use crate::macos::NativeWindow as CompanionWindow;
#[cfg(not(target_os = "macos"))]
pub(crate) use tauri::WebviewWindow as CompanionWindow;
