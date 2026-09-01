use tauri::{Manager, Runtime, WebviewWindow, WebviewWindowBuilder};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWNOACTIVATE, ShowWindow};

pub(crate) fn configure<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

pub(crate) fn show_without_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let window = window.clone();
    window.clone().run_on_main_thread(move || {
        let Ok(hwnd) = window.hwnd() else {
            tracing::warn!("failed to access the anchored HWND");
            return;
        };
        // SAFETY: The handle is live, and this code runs on the window thread.
        unsafe {
            let _previous_visibility = ShowWindow(HWND(hwnd.0), SW_SHOWNOACTIVATE);
        }
    })
}

pub(crate) fn hide(window: &WebviewWindow) -> tauri::Result<()> {
    window.hide()
}
