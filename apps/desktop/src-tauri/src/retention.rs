//! Local session-data retention and its low-frequency scheduler.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

use crate::store::{Revision, Store};

const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Start cleanup at launch and repeat it once per day.
pub fn spawn_scheduler(app: &AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        cleanup(app.clone()).await;
        loop {
            tokio::time::sleep(CLEANUP_INTERVAL).await;
            cleanup(app.clone()).await;
        }
    })
}

/// Notify views and refresh derived repository counts after session removal.
/// `revision` is the store revision at or after the purge: every purged row
/// is absent at it.
pub fn note_removed(app: &AppHandle, removed: usize, revision: Revision) {
    if removed == 0 {
        return;
    }
    if let Err(error) = crate::repositories::refresh_session_counts(&app.state::<Store>()) {
        tracing::warn!(event = "retention_repository_counts_failed", error = %error);
    }
    // Retention knows only the count, not the keys: a broad removal. The
    // registry checks every live entry that predates the revision.
    crate::session_lifecycle::report(
        app,
        crate::session_lifecycle::SyncObservation::Removed {
            scope: crate::session_lifecycle::RemovalScope::Broad,
            reason: crate::session_lifecycle::RemovalReason::Purged,
            revision,
        },
    );
}

async fn cleanup(app: AppHandle) {
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let (removed, revision) = worker_app
            .state::<Store>()
            .apply_session_retention(unix_now())?;
        note_removed(&worker_app, removed, revision);
        Ok::<_, anyhow::Error>(())
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(event = "session_retention_failed", error = %error),
        Err(error) => tracing::warn!(event = "session_retention_worker_failed", error = %error),
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
