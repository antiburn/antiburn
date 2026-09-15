//! Applies the stored tray and Dock visibility to the native shell.

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::Manager;

use crate::store::AppSettings;

#[cfg(target_os = "macos")]
const DOCK_HIDE_RETRY: std::time::Duration = std::time::Duration::from_millis(1_100);
#[cfg(target_os = "macos")]
static DESIRED_DOCK_VISIBLE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

#[derive(Debug, PartialEq, Eq)]
struct PresenceTransition {
    tray: Option<bool>,
    dock: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresenceSurface {
    Tray,
    Dock,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct WindowCanHideSnapshot {
    values: Option<Vec<(usize, bool)>>,
}

#[cfg(target_os = "macos")]
impl WindowCanHideSnapshot {
    fn capture(&mut self, values: Vec<(usize, bool)>) {
        let saved = self.values.get_or_insert_default();
        for value in values {
            if !saved.iter().any(|(id, _)| *id == value.0) {
                saved.push(value);
            }
        }
    }

    fn take_for(&mut self, window_ids: &[usize]) -> Vec<(usize, bool)> {
        let Some(values) = self.values.take() else {
            return Vec::new();
        };
        values
            .into_iter()
            .filter(|(id, _)| window_ids.contains(id))
            .collect()
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    static WINDOW_CAN_HIDE: std::cell::RefCell<WindowCanHideSnapshot> =
        std::cell::RefCell::new(WindowCanHideSnapshot::default());
}

impl PresenceTransition {
    fn at_launch(settings: &AppSettings) -> Self {
        Self {
            tray: Some(settings.tray_icon_visible),
            dock: Some(settings.dock_icon_visible),
        }
    }

    fn between(previous: &AppSettings, saved: &AppSettings) -> Self {
        Self {
            tray: (previous.tray_icon_visible != saved.tray_icon_visible)
                .then_some(saved.tray_icon_visible),
            dock: (previous.dock_icon_visible != saved.dock_icon_visible)
                .then_some(saved.dock_icon_visible),
        }
    }
}

/// Applies both values after the native tray exists.
pub fn apply_at_launch(app: &AppHandle, settings: &AppSettings) {
    apply(app, PresenceTransition::at_launch(settings));
}

/// Applies only values that changed in a saved settings transition.
pub fn apply_transition(app: &AppHandle, previous: &AppSettings, saved: &AppSettings) {
    apply(app, PresenceTransition::between(previous, saved));
}

fn apply(app: &AppHandle, transition: PresenceTransition) {
    apply_with(transition, |surface, visible| match surface {
        PresenceSurface::Tray => apply_tray(app, visible),
        PresenceSurface::Dock => apply_dock(app, visible),
    });
}

fn apply_with(
    transition: PresenceTransition,
    mut apply_surface: impl FnMut(PresenceSurface, bool),
) {
    if let Some(visible) = transition.tray {
        apply_surface(PresenceSurface::Tray, visible);
    }
    if let Some(visible) = transition.dock {
        apply_surface(PresenceSurface::Dock, visible);
    }
}

#[cfg(target_os = "macos")]
fn dock_retry_visibility(settings: Option<&AppSettings>) -> bool {
    settings.is_none_or(|settings| settings.dock_icon_visible)
}

fn apply_tray(app: &AppHandle, visible: bool) {
    if let Err(error) = crate::tray::set_visible(app, visible) {
        ::tracing::warn!(event = "tray_visibility_apply_failed", visible, error = %error);
    }
}

#[cfg(target_os = "macos")]
fn native_windows() -> Option<Vec<objc2::rc::Retained<objc2_app_kit::NSWindow>>> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let main_thread = MainThreadMarker::new()?;
    Some(
        NSApplication::sharedApplication(main_thread)
            .windows()
            .iter()
            .collect(),
    )
}

#[cfg(target_os = "macos")]
fn native_window_id(window: &objc2::rc::Retained<objc2_app_kit::NSWindow>) -> usize {
    objc2::rc::Retained::as_ptr(window).addr()
}

#[cfg(target_os = "macos")]
fn capture_window_can_hide() {
    let Some(windows) = native_windows() else {
        ::tracing::warn!(event = "dock_window_state_capture_off_main_thread");
        return;
    };
    let values = windows
        .iter()
        .map(|window| (native_window_id(window), window.canHide()))
        .collect();
    WINDOW_CAN_HIDE.with(|snapshot| snapshot.borrow_mut().capture(values));
}

#[cfg(target_os = "macos")]
fn restore_window_can_hide() {
    // A newer hide cancels the queued window-state restore from a rapid show.
    if !DESIRED_DOCK_VISIBLE.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    let Some(windows) = native_windows() else {
        ::tracing::warn!(event = "dock_window_state_restore_off_main_thread");
        return;
    };
    let window_ids = windows.iter().map(native_window_id).collect::<Vec<_>>();
    let values = WINDOW_CAN_HIDE.with(|snapshot| snapshot.borrow_mut().take_for(&window_ids));
    for window in windows {
        if let Some((_, can_hide)) = values
            .iter()
            .find(|(id, _)| *id == native_window_id(&window))
        {
            window.setCanHide(*can_hide);
        }
    }
}

#[cfg(target_os = "macos")]
fn request_native_dock_visibility(app: &AppHandle, visible: bool) -> bool {
    if !visible {
        capture_window_can_hide();
    }
    if let Err(error) = app.set_dock_visibility(visible) {
        ::tracing::warn!(event = "dock_visibility_apply_failed", visible, error = %error);
        return false;
    }

    if visible && let Err(error) = app.run_on_main_thread(restore_window_can_hide) {
        ::tracing::warn!(event = "dock_window_state_restore_schedule_failed", error = %error);
    }
    true
}

#[cfg(target_os = "macos")]
fn apply_dock(app: &AppHandle, visible: bool) {
    DESIRED_DOCK_VISIBLE.store(visible, std::sync::atomic::Ordering::Release);
    if !request_native_dock_visibility(app, visible) || visible {
        return;
    }

    // Tauri ignores a Dock hide for one second after a show.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(DOCK_HIDE_RETRY).await;
        let retry_app = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            let settings = Some(retry_app.state::<crate::store::Store>().settings_snapshot());
            let visible = dock_retry_visibility(settings.as_ref());
            DESIRED_DOCK_VISIBLE.store(visible, std::sync::atomic::Ordering::Release);
            request_native_dock_visibility(&retry_app, visible);
        }) {
            ::tracing::warn!(event = "dock_visibility_retry_schedule_failed", error = %error);
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn apply_dock(_app: &AppHandle, _visible: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_presence_needs_no_native_transition() {
        let settings = AppSettings::default();
        assert_eq!(
            PresenceTransition::between(&settings, &settings),
            PresenceTransition {
                tray: None,
                dock: None,
            }
        );
    }

    #[test]
    fn launch_applies_both_stored_presence_values() {
        let settings = AppSettings {
            tray_icon_visible: false,
            dock_icon_visible: true,
            ..AppSettings::default()
        };
        assert_eq!(
            PresenceTransition::at_launch(&settings),
            PresenceTransition {
                tray: Some(false),
                dock: Some(true),
            }
        );
    }

    #[test]
    fn launch_dispatches_both_native_surface_updates() {
        let mut updates = Vec::new();

        apply_with(
            PresenceTransition {
                tray: Some(false),
                dock: Some(true),
            },
            |surface, visible| updates.push((surface, visible)),
        );

        assert_eq!(
            updates,
            vec![
                (PresenceSurface::Tray, false),
                (PresenceSurface::Dock, true),
            ]
        );
    }

    #[test]
    fn presence_transition_carries_only_changed_values() {
        let previous = AppSettings::default();
        let saved = AppSettings {
            tray_icon_visible: false,
            ..previous.clone()
        };

        assert_eq!(
            PresenceTransition::between(&previous, &saved),
            PresenceTransition {
                tray: Some(false),
                dock: None,
            }
        );
    }

    #[test]
    fn dock_transition_carries_the_saved_visibility() {
        let previous = AppSettings::default();
        let saved = AppSettings {
            dock_icon_visible: false,
            ..previous.clone()
        };

        assert_eq!(
            PresenceTransition::between(&previous, &saved),
            PresenceTransition {
                tray: None,
                dock: Some(false),
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dock_retry_uses_the_latest_stored_state_and_fails_visible() {
        let hidden = AppSettings {
            dock_icon_visible: false,
            ..AppSettings::default()
        };
        let visible = AppSettings::default();

        assert!(!dock_retry_visibility(Some(&hidden)));
        assert!(dock_retry_visibility(Some(&visible)));
        assert!(dock_retry_visibility(None));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dock_round_trip_restores_each_existing_windows_hide_behavior() {
        let mut snapshot = WindowCanHideSnapshot::default();
        snapshot.capture(vec![(10, true), (20, false)]);
        snapshot.capture(vec![(10, false), (20, true), (30, true)]);

        assert_eq!(
            snapshot.take_for(&[10, 20, 30]),
            vec![(10, true), (20, false), (30, true)]
        );
        assert!(snapshot.take_for(&[10, 20]).is_empty());
    }
}
