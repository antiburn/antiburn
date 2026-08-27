//! Which operating-system-protected directories the user has allowed.
//!
//! This is the application's half of the engine's [`ConsentGrants`] seam. The
//! engine decides *when* consent matters; this decides where the answer is
//! kept, which is the app database — the same place every other durable
//! decision lives.
//!
//! # The rule that governs everything here
//!
//! **Never read a protected directory to find out whether we may read it.**
//! That read is exactly what raises the system dialog, so a background pass
//! doing it produces a permission prompt with nothing on screen to explain it.
//! Consequently:
//!
//! - [`granted_dirs`](ConsentGrants::granted_dirs) reports what was recorded and
//!   probes nothing.
//! - [`revoke_grant_covering`](ConsentGrants::revoke_grant_covering) is
//!   *reactive*: it runs after a read already came back denied, so it learns a
//!   grant is stale without ever asking.
//! - [`probe_and_record`](StoreConsentGrants::probe_and_record) and
//!   [`discover_external_grants`](ConsentGrants::discover_external_grants) are
//!   the only paths that can prompt, and both are reachable only from an
//!   explicit user action.
//!
//! # Why the record can be wrong, and why that is fine
//!
//! The system is authoritative and revokes grants without telling us — a
//! settings change or a permissions reset invalidates a row with no
//! notification. So this is a cache of the user's decisions, kept honest
//! lazily: the next denied read drops the row. Keeping it eagerly correct would
//! mean probing, which is the one thing that must not happen in the background.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use antiburn_local::paths::{home_dir, protected};
use antiburn_local::repositories::{ConsentGrants, probe_path_access};
use async_trait::async_trait;
use serde::Serialize;

use crate::store::Store;

/// Probe outcomes kept for diagnostics. Small on purpose: this answers "what
/// happened the last few times we asked", it is not a log.
const PROBE_HISTORY: usize = 32;

/// A denial faster than this means the system answered from a decision it had
/// already recorded: no dialog was shown, and asking again will show none
/// either. Anything slower means a dialog appeared and the user answered it.
///
/// The threshold is generous because it only has to separate "answered from
/// memory" (microseconds to a few milliseconds) from "a human read a dialog and
/// clicked" (hundreds of milliseconds at the very least).
pub const RECORDED_DENIAL_MS: u64 = 500;

/// Diagnostics live for the run, not in the database: they describe what the
/// system did, which is not a decision worth persisting.
static PROBE_LOG: OnceLock<Mutex<VecDeque<ProbeRecord>>> = OnceLock::new();

fn probe_log() -> &'static Mutex<VecDeque<ProbeRecord>> {
    PROBE_LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(PROBE_HISTORY)))
}

/// One recorded attempt to read a protected directory.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRecord {
    /// The path the probe targeted.
    pub target: String,
    /// What came back.
    pub outcome: String,
    /// How long the system took to answer. See [`RECORDED_DENIAL_MS`].
    pub elapsed_ms: u64,
}

/// The recent probe history, oldest first.
pub fn recent_probes() -> Vec<ProbeRecord> {
    let probes = match probe_log().lock() {
        Ok(probes) => probes,
        Err(poisoned) => poisoned.into_inner(),
    };
    probes.iter().cloned().collect()
}

/// What a deliberate probe found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The directory was readable. The grant may be recorded.
    Granted { elapsed_ms: u64 },
    /// The system refused, and a dialog was shown to ask.
    Denied { elapsed_ms: u64 },
    /// The system refused from a decision it already held, without asking.
    /// Prompting again is futile; the user has to change it in system settings.
    RecordedDenial { elapsed_ms: u64 },
}

impl ProbeOutcome {
    fn classify(granted: bool, elapsed_ms: u64) -> Self {
        match (granted, elapsed_ms < RECORDED_DENIAL_MS) {
            (true, _) => Self::Granted { elapsed_ms },
            (false, true) => Self::RecordedDenial { elapsed_ms },
            (false, false) => Self::Denied { elapsed_ms },
        }
    }

    /// Short label for diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::Denied { .. } => "denied",
            Self::RecordedDenial { .. } => "recorded-denial",
        }
    }

    /// How long the system took to answer.
    pub fn elapsed_ms(self) -> u64 {
        match self {
            Self::Granted { elapsed_ms }
            | Self::Denied { elapsed_ms }
            | Self::RecordedDenial { elapsed_ms } => elapsed_ms,
        }
    }
}

/// The app database's answer to the engine's consent questions.
///
/// Borrows the store rather than owning it: the store is application state with
/// one owner, and a consent view is built per pass.
pub struct StoreConsentGrants<'a> {
    store: &'a Store,
}

impl<'a> StoreConsentGrants<'a> {
    /// View the app database as a consent record.
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Read `path` once and record what happened.
    ///
    /// **This can raise the consent dialog** — that is how a grant is obtained.
    /// Call it only from an explicit user action, with the window already
    /// focused, or the dialog opens behind an app that has no visible window.
    pub async fn probe_and_record(&self, path: &Path) -> ProbeOutcome {
        let started = Instant::now();
        let granted = probe_path_access(path).await;
        // Measured before anything else runs. Opening a settings pane or
        // touching the database first would inflate the one number that
        // separates "the user answered a dialog" from "the system answered from
        // a decision it already had".
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        let outcome = ProbeOutcome::classify(granted, elapsed_ms);
        self.record_probe(&path.to_string_lossy(), outcome.label(), elapsed_ms);
        outcome
    }

    /// Record that the user granted a protected directory.
    pub fn grant(&self, dir_name: &str) -> anyhow::Result<()> {
        self.store.grant_dir(dir_name)
    }
}

#[async_trait]
impl ConsentGrants for StoreConsentGrants<'_> {
    fn granted_dirs(&self) -> HashSet<String> {
        // A database error must never read as "the user granted everything", so
        // failure means the empty set: directories get deferred, not read.
        self.store.granted_dirs().unwrap_or_default()
    }

    async fn discover_external_grants(&self, dir_names: &HashSet<String>) -> HashSet<String> {
        let Some(home) = home_dir() else {
            return HashSet::new();
        };

        let mut discovered = HashSet::new();
        for dir_name in dir_names {
            let outcome = self.probe_and_record(&home.join(dir_name)).await;
            if matches!(outcome, ProbeOutcome::Granted { .. }) && self.grant(dir_name).is_ok() {
                discovered.insert(dir_name.clone());
            }
        }
        discovered
    }

    async fn revoke_grant_covering(&self, path: &Path) -> Option<String> {
        let dir_name = protected::protected_dir_name(path)?;
        self.store.revoke_dir_grant(&dir_name).ok()?;
        Some(dir_name)
    }

    fn record_probe(&self, target: &str, outcome: &str, elapsed_ms: u64) {
        let record = ProbeRecord {
            target: target.to_string(),
            outcome: outcome.to_string(),
            elapsed_ms,
        };
        let mut probes = match probe_log().lock() {
            Ok(probes) => probes,
            Err(poisoned) => poisoned.into_inner(),
        };
        if probes.len() == PROBE_HISTORY {
            probes.pop_front();
        }
        probes.push_back(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Store) {
        let dir = TempDir::new().expect("temp dir");
        let store = Store::open_in_memory(dir.path()).expect("open store");
        (dir, store)
    }

    #[test]
    fn grants_round_trip_through_the_store() {
        let (_dir, store) = store();
        let consent = StoreConsentGrants::new(&store);
        assert!(consent.granted_dirs().is_empty());

        consent.grant("Documents").unwrap();
        assert_eq!(
            consent.granted_dirs(),
            HashSet::from(["Documents".to_string()])
        );

        // Re-granting refreshes the row rather than adding a second.
        consent.grant("Documents").unwrap();
        assert_eq!(consent.granted_dirs().len(), 1);
    }

    #[tokio::test]
    async fn a_denied_read_drops_the_grant_that_covered_it() {
        let (_dir, store) = store();
        let consent = StoreConsentGrants::new(&store);
        let (Some(home), Some(dir_name)) = (home_dir(), protected::protected_dir_names().first())
        else {
            // No consent-controlled directories on this platform.
            return;
        };
        consent.grant(dir_name).unwrap();

        let revoked = consent
            .revoke_grant_covering(&home.join(dir_name).join("repo"))
            .await;

        assert_eq!(revoked.as_deref(), Some(*dir_name));
        assert!(consent.granted_dirs().is_empty());
    }

    #[tokio::test]
    async fn an_unprotected_path_revokes_nothing() {
        let (_dir, store) = store();
        let consent = StoreConsentGrants::new(&store);
        consent.grant("Documents").unwrap();

        assert_eq!(
            consent.revoke_grant_covering(Path::new("/tmp/repo")).await,
            None
        );
        assert_eq!(consent.granted_dirs().len(), 1);
    }

    #[test]
    fn a_fast_denial_reads_as_recorded_rather_than_asked() {
        assert_eq!(
            ProbeOutcome::classify(false, 3),
            ProbeOutcome::RecordedDenial { elapsed_ms: 3 }
        );
        assert_eq!(
            ProbeOutcome::classify(false, RECORDED_DENIAL_MS),
            ProbeOutcome::Denied {
                elapsed_ms: RECORDED_DENIAL_MS
            }
        );
        // A grant is a grant however fast it came back.
        assert_eq!(
            ProbeOutcome::classify(true, 1),
            ProbeOutcome::Granted { elapsed_ms: 1 }
        );
    }

    #[test]
    fn the_probe_history_is_bounded_and_keeps_the_newest() {
        let (_dir, store) = store();
        let consent = StoreConsentGrants::new(&store);
        for index in 0..PROBE_HISTORY + 10 {
            consent.record_probe(&format!("dir-{index}"), "denied", index as u64);
        }

        let probes: Vec<ProbeRecord> = probe_log().lock().unwrap().iter().cloned().collect();
        assert_eq!(probes.len(), PROBE_HISTORY);
        assert_eq!(
            probes[PROBE_HISTORY - 1].target,
            format!("dir-{}", PROBE_HISTORY + 9)
        );
    }
}
