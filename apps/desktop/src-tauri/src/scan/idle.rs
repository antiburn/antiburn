//! Idle expiry: the moment a session's active pill clears on its own.
//!
//! [`analysis::is_active`](crate::analysis::is_active) computes `is_active`
//! at read time from `updated_at_epoch`, so nothing marks the instant a
//! session crosses [`ACTIVE_SESSION_WINDOW_SECS`]. Without this task, a row
//! already on screen would keep its active pill until the next full list
//! refetch. This task sleeps until the next session's window ends and
//! announces the row that just went idle, through the same
//! `SESSION_ENTRY_CHANGED_EVENT` a fresh write announces.

use std::time::Duration;

use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::dto::ActivityEntry;
use crate::store::Store;

/// Slack added past a session's computed deadline, so the task never wakes a
/// moment early and finds the session still (barely) active.
const EXPIRY_SLACK_SECS: i64 = 1;

/// Rearms the idle task's deadline. [`crate::scan::pass`] notifies this after
/// every successful upsert, so a session the task was not yet watching, or
/// one whose activity just moved its own deadline later, is picked up
/// immediately instead of at the task's next stale wake.
#[derive(Default)]
pub struct IdleWake(Notify);

/// Start the idle-expiry loop. The returned handle is aborted with the rest
/// of the schedulers on exit.
pub fn spawn(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let store: Store = (*app.state::<Store>()).clone();
        let announce_app = app.clone();
        let announce = move |entry: ActivityEntry| {
            let _ = announce_app.emit(crate::commands::SESSION_ENTRY_CHANGED_EVENT, &entry);
        };
        // `now` tracks tokio's own clock rather than the wall clock directly,
        // so a test can drive it deterministically under
        // `tokio::time::pause()`: every `tokio::time::sleep` below advances
        // this clock exactly as far as it advances the real one.
        let base_epoch = crate::scan::unix_now();
        let base_instant = Instant::now();
        let now = move || base_epoch + base_instant.elapsed().as_secs() as i64;
        run(&store, app.state::<IdleWake>().inner(), &now, &announce).await;
    })
}

/// Ask the idle task to recompute its deadline now, rather than at its next
/// wake.
pub fn wake(app: &AppHandle) {
    app.state::<IdleWake>().0.notify_one();
}

/// The loop [`spawn`] runs forever. Split out so a test can drive it with a
/// temp [`Store`] and a captured `announce`, without a Tauri app.
async fn run(
    store: &Store,
    wake: &IdleWake,
    now: &(dyn Fn() -> i64 + Send + Sync),
    announce: &(dyn Fn(ActivityEntry) + Send + Sync),
) {
    loop {
        let active = store
            .sessions_active_since(now() - ACTIVE_SESSION_WINDOW_SECS)
            .unwrap_or_default();
        let Some(deadline) = active
            .iter()
            .map(|(_, updated_at)| updated_at + ACTIVE_SESSION_WINDOW_SECS)
            .min()
        else {
            // Nothing to expire: wait for a write that adds or moves one.
            wake.0.notified().await;
            continue;
        };
        let wait_secs = (deadline + EXPIRY_SLACK_SECS - now()).max(0);
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(wait_secs as u64)) => {}
            () = wake.0.notified() => {
                // A write may have moved this deadline; re-read the active
                // set from the top rather than trusting the stale one.
                continue;
            }
        }
        let now = now();
        for (key, updated_at) in &active {
            if now - updated_at < ACTIVE_SESSION_WINDOW_SECS {
                continue;
            }
            if let Some(entry) = crate::insights_worker::completion_entry(store, key, now) {
                announce(entry);
            }
        }
    }
}

#[cfg(test)]
mod tests;
