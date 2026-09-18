//! Shell policy for the floating usage HUD.
//!
//! The `antiburn-hud` crate owns the window mechanism. This module keeps the
//! engine-specific activity lookup and its cost bound inside the shell. It also
//! owns where the HUD's remembered position is kept, and the watcher that
//! reacts when a display connects or disconnects.

#[cfg(target_os = "macos")]
use std::time::Duration;

use antiburn_hud::{DockSettings, Placement};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri::Manager;

use crate::store::Store;

/// The internal scalar holding every remembered HUD position.
const PLACEMENTS_KEY: &str = "internal:hudPlacements";

/// The internal scalar that says the reader wants the HUD on screen. The shell
/// reads it at launch, so the HUD returns before any webview mounts.
const ENABLED_KEY: &str = "internal:hudEnabled";

/// The internal scalar that says whether the HUD was docked, and where.
const DOCK_KEY: &str = "internal:hudDock";

/// The shape of the stored value. A different number means a value this build
/// cannot read, and the HUD starts again from its default position.
const PLACEMENTS_VERSION: u32 = 1;

/// Displays the list remembers. A desk does not have nine of them, and an
/// unbounded list in a settings row is a slow leak.
const MAX_PLACEMENTS: usize = 8;

/// How often the watcher looks for a change in the connected displays.
#[cfg(target_os = "macos")]
const DISPLAY_POLL: Duration = Duration::from_secs(2);

/// The stored value: remembered placements, newest display first.
#[derive(Serialize, Deserialize)]
struct StoredPlacements {
    version: u32,
    entries: Vec<Placement>,
}

/// Every remembered placement, newest display first.
pub fn load_placements(store: &Store) -> Vec<Placement> {
    parse_placements(store.internal_value(PLACEMENTS_KEY).as_deref())
}

/// Whether the reader last left the HUD on. Absent means off.
#[cfg(target_os = "macos")]
pub fn load_enabled(store: &Store) -> bool {
    store.internal_value(ENABLED_KEY).as_deref() == Some("true")
}

/// Remember whether the HUD should return at the next launch.
pub fn save_enabled(store: &Store, enabled: bool) {
    store.set_internal_value(ENABLED_KEY, if enabled { "true" } else { "false" });
}

/// The stored dock state. Anything unreadable means "not docked".
pub fn load_dock(store: &Store) -> DockSettings {
    store
        .internal_value(DOCK_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn save_dock(store: &Store, settings: DockSettings) {
    if let Ok(raw) = serde_json::to_string(&settings) {
        store.set_internal_value(DOCK_KEY, &raw);
    }
}

/// Bring the HUD back at launch when the reader left it on. The popover used
/// to do this, but the popover is lazy, so the HUD waited for the first click
/// on the menu bar.
#[cfg(target_os = "macos")]
pub fn restore_at_launch(app: &AppHandle) {
    let store = app.state::<Store>();
    if !load_enabled(&store) {
        return;
    }
    let entries = load_placements(&store);
    crate::analytics::prepare_hud_exposure(crate::analytics::event::Origin::Automatic);
    if let Err(error) = antiburn_hud::open(app, &entries) {
        crate::analytics::cancel_hud_exposure();
        ::tracing::warn!(event = "hud_launch_restore_failed", error = %error);
        return;
    }
    antiburn_hud::restore_dock(app, load_dock(&store));
}

/// Remember one position and make its display the preferred one.
pub fn save_placement(store: &Store, placement: Placement) {
    let entries = promote(load_placements(store), placement);
    let stored = StoredPlacements {
        version: PLACEMENTS_VERSION,
        entries,
    };
    if let Ok(raw) = serde_json::to_string(&stored) {
        store.set_internal_value(PLACEMENTS_KEY, &raw);
    }
}

/// Move the HUD when a display connects or disconnects.
///
/// The watcher only reads the remembered list. A display that goes away makes
/// the HUD borrow another one; it must not make that other display the
/// preferred one, or the HUD would stay there when the first display returns.
///
/// A poll rather than an event: it is the pattern the hover watcher in the HUD
/// crate already uses, and it needs no platform notification of its own. The
/// poll costs nothing while the HUD is closed.
#[cfg(target_os = "macos")]
pub fn spawn_display_watcher(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut visibility = antiburn_hud::visibility_receiver();
        let mut connected: Vec<String> = Vec::new();
        loop {
            if !visibility.borrow_and_update().is_visible() {
                if visibility.changed().await.is_err() {
                    break;
                }
                continue;
            }
            tokio::select! {
                () = tokio::time::sleep(DISPLAY_POLL) => {}
                changed = visibility.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    continue;
                }
            }
            // A notch can come back without the display list changing: the
            // safe area of a display settles after an arrangement change, and
            // the HUD waits at the top edge until it does.
            if antiburn_hud::island_wanted_off_notch() {
                let _ = crate::main_window::on_main_value(&app, |_| antiburn_hud::refresh_notch())
                    .await;
                antiburn_hud::reclaim_island(&app);
            }
            let now = antiburn_hud::monitor_keys(&app);
            if now == connected {
                continue;
            }
            connected = now;
            // The notch may have come or gone with the display.
            let _ =
                crate::main_window::on_main_value(&app, |_| antiburn_hud::refresh_notch()).await;
            let entries = load_placements(&app.state::<Store>());
            if let Err(error) = antiburn_hud::apply_placement(&app, &entries) {
                ::tracing::warn!(event = "hud_display_change_move_failed", error = %error);
            }
        }
    });
}

/// Keep the display watcher absent where the HUD is unavailable.
#[cfg(not(target_os = "macos"))]
pub fn spawn_display_watcher(_app: &AppHandle) {}

/// Read the stored value. Anything unreadable means "no memory yet": a bad row
/// must never stop the HUD from appearing.
fn parse_placements(raw: Option<&str>) -> Vec<Placement> {
    raw.and_then(|raw| serde_json::from_str::<StoredPlacements>(raw).ok())
        .filter(|stored| stored.version == PLACEMENTS_VERSION)
        .map(|stored| stored.entries)
        .unwrap_or_default()
}

/// Put one placement at the head, replacing any earlier entry for its display.
fn promote(entries: Vec<Placement>, placement: Placement) -> Vec<Placement> {
    let mut promoted: Vec<Placement> = entries
        .into_iter()
        .filter(|entry| entry.monitor != placement.monitor)
        .collect();
    promoted.insert(0, placement);
    promoted.truncate(MAX_PLACEMENTS);
    promoted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(monitor: &str, x: f64, y: f64) -> Placement {
        Placement {
            monitor: monitor.to_string(),
            x,
            y,
        }
    }

    #[test]
    fn a_new_display_goes_to_the_head() {
        let entries = promote(
            vec![placement("laptop", 8.0, 8.0)],
            placement("external", 100.0, 40.0),
        );
        assert_eq!(
            entries,
            vec![
                placement("external", 100.0, 40.0),
                placement("laptop", 8.0, 8.0)
            ]
        );
    }

    #[test]
    fn a_display_already_remembered_moves_to_the_head_once() {
        let entries = promote(
            vec![
                placement("external", 100.0, 40.0),
                placement("laptop", 8.0, 8.0),
            ],
            placement("laptop", 20.0, 60.0),
        );
        assert_eq!(
            entries,
            vec![
                placement("laptop", 20.0, 60.0),
                placement("external", 100.0, 40.0)
            ]
        );
    }

    #[test]
    fn the_list_drops_the_oldest_display_at_its_cap() {
        let mut entries = Vec::new();
        for index in 0..MAX_PLACEMENTS {
            entries = promote(entries, placement(&format!("display-{index}"), 0.0, 0.0));
        }
        entries = promote(entries, placement("newest", 0.0, 0.0));
        assert_eq!(entries.len(), MAX_PLACEMENTS);
        assert_eq!(entries[0].monitor, "newest");
        assert!(entries.iter().all(|entry| entry.monitor != "display-0"));
    }

    #[test]
    fn a_stored_value_round_trips() {
        let stored = StoredPlacements {
            version: PLACEMENTS_VERSION,
            entries: vec![placement("laptop", 8.0, 8.0)],
        };
        let raw = serde_json::to_string(&stored).expect("serialize");
        assert_eq!(
            parse_placements(Some(&raw)),
            vec![placement("laptop", 8.0, 8.0)]
        );
    }

    #[test]
    fn an_unreadable_value_means_no_memory_yet() {
        assert!(parse_placements(None).is_empty());
        assert!(parse_placements(Some("")).is_empty());
        assert!(parse_placements(Some("{\"version\":1}")).is_empty());
        assert!(parse_placements(Some("{\"version\":99,\"entries\":[]}")).is_empty());
    }
}
