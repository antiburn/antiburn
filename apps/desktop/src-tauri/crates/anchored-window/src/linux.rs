use gtk::prelude::*;
use tauri::{Manager, Runtime, WebviewWindow, WebviewWindowBuilder};

pub(crate) fn configure<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

pub(crate) fn show_without_activation(window: &WebviewWindow) -> tauri::Result<()> {
    let window = window.clone();
    window.clone().run_on_main_thread(move || {
        let Ok(gtk_window) = window.gtk_window() else {
            tracing::warn!("failed to access the anchored GTK window");
            return;
        };
        gtk_window.set_focus_on_map(false);
        if let Err(error) = window.show() {
            tracing::warn!(%error, "failed to show the anchored GTK window");
        }
    })
}

pub(crate) fn hide(window: &WebviewWindow) -> tauri::Result<()> {
    window.hide()
}
