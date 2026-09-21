//! The IPC surface of the usage HUD: the overlay window, its island, and
//! its hover detail. The commands stay thin, as in [`crate::commands`].

use tauri::Manager;

use crate::commands::{CommandResult, fail, run_blocking};
use crate::store::Store;

/// Open or re-show the always-on-top usage HUD.
#[tauri::command]
pub async fn open_overlay_window(
    app: tauri::AppHandle,
    origin: crate::analytics::event::Origin,
) -> CommandResult<()> {
    let store = app.state::<Store>().inner().clone();
    let (entries, dock) = run_blocking(move || {
        // Every open means the reader wants the HUD back at the next launch.
        crate::hud::save_enabled(&store, true);
        Ok((
            crate::hud::load_placements(&store),
            crate::hud::load_dock(&store),
        ))
    })
    .await?;
    let needs_exposure = hud_needs_exposure(&app);
    if needs_exposure {
        crate::analytics::prepare_hud_exposure(origin);
    }
    if let Err(error) = antiburn_hud::open(&app, &entries) {
        if needs_exposure {
            crate::analytics::cancel_hud_exposure();
        }
        return Err(fail(error));
    }
    // A HUD the reader left docked comes back docked. The island reads the
    // notch from AppKit, which is main-thread work.
    crate::main_window::on_main_value(&app, move |app| antiburn_hud::restore_dock(app, dock))
        .await?;
    Ok(())
}

/// Log why a HUD drag ended. The webview cannot write the shell log itself.
///
/// `reason` is the DOM event type that ended the drag. `origin_known` is
/// false when the drag ended before the webview read the window position.
#[tauri::command]
pub fn hud_drag_ended(reason: String, origin_known: bool) {
    antiburn_hud::end_drag();
    ::tracing::info!(event = "hud_drag_ended", reason, origin_known);
}

/// Free a docked HUD: a drag started on it.
///
/// While the drag runs, the HUD previews the island when a drop would make
/// one.
#[tauri::command]
pub fn tear_off_overlay(app: tauri::AppHandle) -> bool {
    let was_docked = antiburn_hud::begin_drag(&app);
    crate::hud::save_dock(&app.state::<Store>(), antiburn_hud::dock_settings());
    was_docked
}

/// Whether a connected display has a notch for the HUD to sit in.
#[tauri::command]
pub fn hud_island_available() -> bool {
    antiburn_hud::refresh_notch()
}

/// What the island is doing now, for a HUD webview that just mounted.
#[tauri::command]
pub fn hud_island_state() -> antiburn_hud::IslandState {
    antiburn_hud::island_state()
}

/// Open a collapsed island now: a mouse-down on it. It lingers as a peek does.
#[tauri::command]
pub fn expand_hud_island(app: tauri::AppHandle) {
    antiburn_hud::expand_island(&app);
}

/// Put the HUD in the notch, or take it out and float it at its last place.
///
/// Returns the dock state after the change, for the webview's toggle.
#[tauri::command]
pub fn set_hud_island(app: tauri::AppHandle, on: bool) -> antiburn_hud::DockSettings {
    let store = app.state::<Store>();
    if on {
        antiburn_hud::island_overlay(&app);
    } else if antiburn_hud::tear_off(&app) {
        let entries = crate::hud::load_placements(&store);
        if let Err(error) = antiburn_hud::apply_placement(&app, &entries) {
            ::tracing::warn!(event = "hud_island_leave_move_failed", error = %error);
        }
    }
    let dock = antiburn_hud::dock_settings();
    crate::hud::save_dock(&store, dock);
    dock
}

/// Bring a docked HUD back for a while. `reason` is logged for tuning.
#[tauri::command]
pub fn wake_overlay(app: tauri::AppHandle, reason: String) {
    antiburn_hud::wake_overlay(&app, &reason);
}

#[cfg(target_os = "macos")]
fn hud_needs_exposure(app: &tauri::AppHandle) -> bool {
    !hud_is_exposed(app)
}

#[cfg(not(target_os = "macos"))]
fn hud_needs_exposure(_app: &tauri::AppHandle) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn hud_is_exposed(app: &tauri::AppHandle) -> bool {
    app.get_webview_window(antiburn_hud::OVERLAY_LABEL)
        .is_some_and(|window| window.is_visible().unwrap_or(false))
}

#[cfg(not(target_os = "macos"))]
fn hud_is_exposed(_app: &tauri::AppHandle) -> bool {
    false
}

/// Take the origin after the HUD confirms that it reached the screen.
#[tauri::command]
pub fn take_hud_analytics_origin(app: tauri::AppHandle) -> Option<crate::analytics::event::Origin> {
    crate::analytics::take_hud_exposure_origin(hud_is_exposed(&app))
}

/// Remember where the HUD is, after a drag moved it.
///
/// No argument: the webview knows a drag ended, the shell knows where the
/// window is, and that split keeps geometry out of the IPC payload.
#[tauri::command]
pub async fn record_hud_position(app: tauri::AppHandle) -> CommandResult<()> {
    let placement =
        crate::main_window::on_main_value(&app, antiburn_hud::current_placement).await?;
    // A drop against a display edge docks the HUD there. The placement
    // saved first is the drop, which the dock's home clamps on screen.
    let dock = crate::main_window::on_main_value(&app, antiburn_hud::settle_after_drag).await?;
    let store = app.state::<Store>().inner().clone();
    run_blocking(move || {
        if let Some(placement) = placement {
            crate::hud::save_placement(&store, placement);
        }
        crate::hud::save_dock(&store, dock);
        Ok(())
    })
    .await
}

/// Hide the usage HUD and cancel any pending reveal.
#[tauri::command]
pub fn hide_overlay_window(app: tauri::AppHandle) -> CommandResult<()> {
    crate::analytics::cancel_hud_exposure();
    crate::hud::save_enabled(&app.state::<Store>(), false);
    antiburn_hud::hide(&app).map_err(fail)
}

/// Return whether the HUD should run while its retained renderer mounts.
#[tauri::command]
pub fn is_overlay_work_active() -> bool {
    antiburn_hud::work_is_active()
}

/// Match the native HUD frame to the rendered panel.
#[tauri::command]
pub fn resize_overlay_window(
    app: tauri::AppHandle,
    height: f64,
    anchor_bottom: bool,
    animate: bool,
) -> CommandResult<()> {
    antiburn_hud::resize(&app, height, anchor_bottom, animate).map_err(fail)
}

/// Request the hover detail window with the newest usage payload.
///
/// The payload passes through opaque on purpose: the HUD webview produces it
/// and the detail webview consumes it, so the shell does not model its shape.
#[tauri::command]
pub fn show_hud_detail(app: tauri::AppHandle, state: serde_json::Value) {
    antiburn_hud::show_detail(&app, state);
}

/// Hide the hover detail window.
#[tauri::command]
pub fn hide_hud_detail(app: tauri::AppHandle) {
    antiburn_hud::hide_detail(&app);
}

/// Hide the detail window now that its webview cleared the card.
#[tauri::command]
pub fn conceal_hud_detail(app: tauri::AppHandle) {
    antiburn_hud::conceal_detail(&app);
}

/// Return the newest detail payload for a detail webview that mounts late.
#[tauri::command]
pub fn get_hud_detail_state() -> serde_json::Value {
    antiburn_hud::detail_state()
}

/// Size and place the detail window from its webview's measured height.
#[tauri::command]
pub fn set_hud_detail_size(app: tauri::AppHandle, height: f64) {
    antiburn_hud::apply_detail_size(&app, height);
}
