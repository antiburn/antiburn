//! Device upload preference writes under the shared Cloud work permit.

use super::{Controller, DestinationBinding, Engine, Ordering, Scheduler, Store};

const CHANGED: &str = "Cloud account or upload controls changed. Try again.";
const SAVE_FAILED: &str = "Could not save Cloud upload controls.";

/// Keep OFF independent of account access. Release the engine lock before the
/// caller reads status, which also needs that lock.
pub(super) fn save_switch(
    controller: &Controller,
    store: &Store,
    enabled: bool,
    binding: Option<&DestinationBinding>,
    expected_pause: u64,
    account_generation: u64,
) -> Result<(), String> {
    let scheduler = &controller.sync;
    if !enabled {
        if scheduler.device_pause.load(Ordering::Acquire) != expected_pause {
            return Err(CHANGED.into());
        }
        store
            .set_cloud_device_upload_enabled(false)
            .map_err(|_| SAVE_FAILED)?;
        return scheduler.finish_device_change(expected_pause);
    }

    let binding = binding.ok_or("Connect to your Personal workspace to turn on Cloud uploads.")?;
    let engine = controller
        .engine
        .lock()
        .map_err(|_| "Cloud is unavailable.")?;
    resume(
        scheduler,
        store,
        &engine,
        binding,
        expected_pause,
        account_generation,
        || controller.account_generation.load(Ordering::Acquire) == account_generation,
    )
}

/// Accept ON before the durable commit. The device CAS orders ON against OFF.
/// The account recheck rejects a sign-out that races with that CAS.
pub(super) fn resume(
    scheduler: &Scheduler,
    store: &Store,
    engine: &Engine,
    binding: &DestinationBinding,
    expected_pause: u64,
    account_generation: u64,
    account_current: impl Fn() -> bool,
) -> Result<(), String> {
    let current = || {
        engine.generation == account_generation
            && engine
                .session()
                .is_some_and(|session| binding.matches(session))
            && scheduler.device_pause.load(Ordering::Acquire) == expected_pause
            && account_current()
    };
    if !current() {
        return Err(CHANGED.into());
    }
    match store.resume_cloud_device_uploads(current, || {
        if scheduler.finish_device_change(expected_pause).is_err() {
            return false;
        }
        if account_current() {
            return true;
        }
        // Restore this fence only if a newer OFF has not replaced it.
        let _ = scheduler.device_pause.compare_exchange(
            expected_pause & !1,
            expected_pause,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        false
    }) {
        Ok(true) => Ok(()),
        Ok(false) => Err(CHANGED.into()),
        Err(_) => {
            // Keep admission closed if the store cannot confirm the commit.
            scheduler.device_pause.fetch_or(1, Ordering::AcqRel);
            Err(SAVE_FAILED.into())
        }
    }
}
