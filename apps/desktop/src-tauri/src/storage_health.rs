// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Whether the local database is still accepting writes.
//!
//! Everything antiburn knows lives in one SQLite file, and the ways that file
//! stops working — a full disk, a read-only volume, a damaged page — are all
//! invisible from the outside. The list keeps rendering; it simply stops
//! changing. This module exists so that failure has a voice: the scan records
//! a write failure here, the popover shows a banner, and the retry is an
//! ordinary rescan.
//!
//! Two deliberate boundaries:
//!
//! - **Only writes.** Opening the database is not tracked, because a failure
//!   there is fatal at launch: there is no useful antiburn without a store, and
//!   an app that starts into a permanent error banner is worse than one that
//!   refuses to start. See `docs/deviations.md`.
//! - **Only changes are announced.** The status is emitted when it *changes*,
//!   so a machine whose disk is full does not generate one event per scan tick.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Event the shell emits when storage health changes.
pub const EVENT: &str = "storage:health";

/// What the popover renders about the local database.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageHealthStatus {
    /// True when the last write antiburn attempted did not land.
    pub failing: bool,
    /// What failed, in the store's own words. Present only while `failing`.
    pub message: Option<String>,
}

/// The current answer, registered as Tauri managed state.
#[derive(Default)]
pub struct StorageHealth(Mutex<StorageHealthStatus>);

impl StorageHealth {
    pub fn status(&self) -> StorageHealthStatus {
        self.lock().clone()
    }

    /// Replace the status, returning it only when it actually changed.
    fn replace(&self, next: StorageHealthStatus) -> Option<StorageHealthStatus> {
        let mut current = self.lock();
        if *current == next {
            return None;
        }
        *current = next.clone();
        Some(next)
    }

    /// A poisoned lock still holds a usable status: the panic that poisoned it
    /// happened in a caller.
    fn lock(&self) -> std::sync::MutexGuard<'_, StorageHealthStatus> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What the reader is told a storage failure was.
///
/// `what` names the write in the app's own vocabulary ("the session index"),
/// because a bare SQLite message tells a reader nothing about which part of
/// antiburn stopped working.
pub fn message_for(what: &str, error: &anyhow::Error) -> String {
    format!("{what} could not be written: {error}")
}

/// Record that a write failed. Emits only if this is news.
pub fn note_failure(app: &AppHandle, what: &str, error: &anyhow::Error) {
    let next = StorageHealthStatus {
        failing: true,
        message: Some(message_for(what, error)),
    };
    announce(app, next);
}

/// Record that a write succeeded, clearing any earlier failure.
pub fn note_ok(app: &AppHandle) {
    announce(app, StorageHealthStatus::default());
}

/// The current answer, or a healthy one when the state is not registered (which
/// is what a store-less test harness looks like).
pub fn status(app: &AppHandle) -> StorageHealthStatus {
    app.try_state::<StorageHealth>()
        .map(|health| health.status())
        .unwrap_or_default()
}

fn announce(app: &AppHandle, next: StorageHealthStatus) {
    let Some(health) = app.try_state::<StorageHealth>() else {
        return;
    };
    if let Some(changed) = health.replace(next) {
        let _ = app.emit(EVENT, changed);
    }
}

/// Run a store write, recording a failure against storage health.
///
/// Takes the already-evaluated result rather than a closure: every caller is a
/// single synchronous store call, and threading a closure through would only
/// obscure which line is the write.
pub fn checked<T>(app: &AppHandle, what: &str, result: anyhow::Result<T>) -> anyhow::Result<T> {
    if let Err(error) = &result {
        note_failure(app, what, error);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_healthy_store_reports_nothing_to_show() {
        let health = StorageHealth::default();
        let status = health.status();
        assert!(!status.failing);
        assert!(status.message.is_none());
    }

    #[test]
    fn only_a_change_is_worth_announcing() {
        let health = StorageHealth::default();
        let failing = StorageHealthStatus {
            failing: true,
            message: Some("the session index could not be written: disk full".into()),
        };

        assert!(
            health.replace(failing.clone()).is_some(),
            "the first failure is news"
        );
        assert!(
            health.replace(failing.clone()).is_none(),
            "a scan runs every minute; the same failure is not news sixty times"
        );

        // A *different* failure is news again, because it names something else.
        let other = StorageHealthStatus {
            failing: true,
            message: Some("the retention prune could not be written: database is locked".into()),
        };
        assert!(health.replace(other).is_some());

        assert!(
            health.replace(StorageHealthStatus::default()).is_some(),
            "recovery is news"
        );
        assert!(health.replace(StorageHealthStatus::default()).is_none());
    }

    #[test]
    fn the_message_names_the_part_of_the_app_that_stopped_working() {
        let error = anyhow::anyhow!("attempt to write a readonly database");
        let message = message_for("the session index", &error);
        assert!(message.contains("the session index"));
        assert!(message.contains("readonly"));
    }

    #[test]
    fn the_status_serializes_as_the_shape_the_popover_renders() {
        let json = serde_json::to_value(StorageHealthStatus {
            failing: true,
            message: Some("the session index could not be written: disk full".into()),
        })
        .unwrap();
        assert_eq!(json["failing"], true);
        assert!(json["message"].as_str().unwrap().contains("disk full"));

        let healthy = serde_json::to_value(StorageHealthStatus::default()).unwrap();
        assert_eq!(healthy["failing"], false);
        assert!(healthy["message"].is_null());
    }
}
