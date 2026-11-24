//! Schedule Cloud turns without holding the blocking permit between turns.

use super::{emit_status, turn_team_workspace};
use crate::{
    cloud::{Controller, emit_snapshot, ensure_cloud_setup, snapshot, work::Priority},
    store::Store,
};
use std::{sync::atomic::Ordering, time::Duration};
use tauri::{AppHandle, Manager};
use tokio::{sync::Notify, time::Instant};

const DISCOVERY_CONTINUATION_DELAY: Duration = Duration::from_millis(50);
const SYNC_TICK_DELAY: Duration = Duration::from_secs(2);

/// Start once from Cloud initialization, regardless of renderer lifetime.
/// Busy turns wait for the next regular tick or control change.
pub(in crate::cloud) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut next_tick = Instant::now() + SYNC_TICK_DELAY;
        let mut continue_discovery = false;
        loop {
            let controller = app.state::<Controller>();
            let regular_tick =
                wait_for_sync_tick(&controller.sync.wake, next_tick, continue_discovery).await;
            let content_generation = controller.sync.generation.load(Ordering::Acquire);
            let worker_app = app.clone();
            continue_discovery = controller
                .blocking
                .run(Priority::Normal, move || {
                    run_tick(&worker_app, regular_tick, content_generation)
                })
                .await
                .unwrap_or(false);
            if regular_tick {
                next_tick = Instant::now() + SYNC_TICK_DELAY;
            }
        }
    });
}

/// Let a control change or the regular deadline take priority over discovery.
async fn wait_for_sync_tick(wake: &Notify, next_tick: Instant, continue_discovery: bool) -> bool {
    tokio::select! {
        biased;
        _ = wake.notified() => true,
        _ = tokio::time::sleep_until(next_tick) => true,
        _ = tokio::time::sleep(DISCOVERY_CONTINUATION_DELAY), if continue_discovery => false,
    }
}

/// Run under the shared Cloud permit. A discovery tick skips runners that did
/// not advance an OpenCode slice. Return whether there is another ready slice.
fn run_tick(app: &AppHandle, regular_tick: bool, content_generation: u64) -> Result<bool, String> {
    let controller = app.state::<Controller>();
    let mut engine = controller
        .engine
        .lock()
        .map_err(|_| "Cloud is unavailable.")?;
    let mut runner = controller
        .sync
        .runner
        .lock()
        .map_err(|_| "Cloud sync is unavailable.")?;
    let store = app.state::<Store>();
    let setup_failed = ensure_cloud_setup(&controller.setup_ready, &store).is_err();
    let account_generation = controller.account_generation.load(Ordering::Acquire);
    let engine_generation = engine.generation;
    let current = || {
        controller.account_generation.load(Ordering::Acquire) == account_generation
            && controller.sync.generation.load(Ordering::Acquire) == content_generation
    };
    let account_current = || {
        engine_generation == account_generation
            && controller.account_generation.load(Ordering::Acquire) == account_generation
    };
    let upload_current = || controller.sync.device_admission_open() && current();
    let device_enabled = if setup_failed {
        Err("Could not read Cloud upload controls.".into())
    } else {
        controller.sync.device_upload_enabled(&store)
    };
    let mut host = controller
        .capture
        .lock()
        .map_err(|_| "Session capture is unavailable.")?;
    let mut project_cache = controller
        .project_cache
        .lock()
        .map_err(|_| "Project discovery is unavailable.")?;
    let mut personal_continues = false;
    if regular_tick || runner.discovery_continuation_due() {
        let failed = device_enabled.is_err()
            || runner
                .turn(
                    &store,
                    &mut host,
                    &mut engine,
                    &upload_current,
                    (&account_current, None),
                    &mut project_cache,
                )
                .is_err();
        if current() {
            runner.report_status(
                &store,
                &mut host,
                &engine,
                failed,
                &account_current,
                |status| {
                    emit_status(
                        app,
                        status.with_device_upload_enabled(matches!(device_enabled, Ok(true))),
                    );
                },
            );
            emit_snapshot(app, snapshot(app, &engine));
        }
        personal_continues = !failed && runner.discovery_continuation_due();
    }
    if !matches!(device_enabled, Ok(true)) || !upload_current() || !account_current() {
        return Ok(false);
    }
    let team_continues = turn_team_workspace(
        app,
        &store,
        &mut engine,
        &mut host,
        &mut project_cache,
        (&upload_current, &account_current),
        regular_tick,
    )?;
    Ok(upload_current() && account_current() && (personal_continues || team_continues))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn cloud_scheduler_short_ticks_do_not_delay_regular_ticks_or_control_changes() {
        let wake = Notify::new();
        let started = Instant::now();
        let next_tick = started + SYNC_TICK_DELAY;

        for _ in 0..2 {
            assert!(!wait_for_sync_tick(&wake, next_tick, true).await);
            assert!(Instant::now() < next_tick);
        }
        let before_wake = Instant::now();
        assert!(before_wake - started >= 2 * DISCOVERY_CONTINUATION_DELAY);
        wake.notify_one();
        assert!(wait_for_sync_tick(&wake, next_tick, true).await);
        assert_eq!(Instant::now(), before_wake);

        assert!(wait_for_sync_tick(&wake, next_tick, false).await);
        assert!(Instant::now() >= next_tick);
        assert!(wait_for_sync_tick(&wake, next_tick, true).await);
    }
}
