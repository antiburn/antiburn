//! Shell policy for the floating usage HUD.
//!
//! The `antiburn-hud` crate owns the window mechanism. This module keeps the
//! engine-specific activity lookup and its cost bound inside the shell. It also
//! owns where the HUD's remembered position is kept, and the watcher that
//! reacts when a display connects or disconnects.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use antiburn_hud::Placement;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::store::Store;

/// The internal scalar holding every remembered HUD position.
const PLACEMENTS_KEY: &str = "internal:hudPlacements";

/// The shape of the stored value. A different number means a value this build
/// cannot read, and the HUD starts again from its default position.
const PLACEMENTS_VERSION: u32 = 1;

/// Displays the list remembers. A desk does not have nine of them, and an
/// unbounded list in a settings row is a slow leak.
const MAX_PLACEMENTS: usize = 8;

/// How often the watcher looks for a change in the connected displays.
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

/// Remember where the HUD is now. Called when a drag settles.
pub fn record_position(app: &AppHandle) {
    let Some(placement) = antiburn_hud::current_placement(app) else {
        return;
    };
    save_placement(&app.state::<Store>(), placement);
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
pub fn spawn_display_watcher(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut connected: Vec<String> = Vec::new();
        loop {
            tokio::time::sleep(DISPLAY_POLL).await;
            if app
                .get_webview_window(antiburn_hud::OVERLAY_LABEL)
                .is_none()
            {
                continue;
            }
            let now = antiburn_hud::monitor_keys(&app);
            if now == connected {
                continue;
            }
            connected = now;
            let entries = load_placements(&app.state::<Store>());
            if let Err(error) = antiburn_hud::apply_placement(&app, &entries) {
                ::tracing::warn!(event = "hud_display_change_move_failed", error = %error);
            }
        }
    });
}

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

/// Return the newest recent transcript write as epoch seconds.
pub async fn latest_session_activity() -> Option<i64> {
    static MEMO: Mutex<Option<(Instant, Option<i64>)>> = Mutex::new(None);
    const MEMO_TTL: Duration = Duration::from_secs(60);

    if let Ok(memo) = MEMO.lock()
        && let Some((at, value)) = *memo
        && at.elapsed() < MEMO_TTL
    {
        return value;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let logs = antiburn_local::discovery::Explorers::DISK
        .discover_recent_sessions(now, antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS)
        .await;
    let latest = logs.iter().filter_map(|log| log.updated_at).max();

    if let Ok(mut memo) = MEMO.lock() {
        *memo = Some((Instant::now(), latest));
    }
    latest
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
