// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The cache-rehydration alert: say when a session pays a second time for
//! context the prompt cache had already stored.
//!
//! # What counts as new
//!
//! The engine reports how many turns of a session rewrote a context the cache
//! already held ([`SessionMetrics::cache_rehydration_count`]). This module
//! keeps the count each session last reported, and announces only an
//! *increase*.
//!
//! A session this module has never seen records its count and says nothing.
//! That one rule is what keeps the first scan of a machine quiet: months of
//! finished sessions each establish a baseline instead of each announcing
//! their history. The reader hears about what happens while antiburn watches.
//!
//! # Why a key/value blob, and not a column
//!
//! "Have I mentioned this yet" is bookkeeping, not session evidence, so it
//! does not belong in the analysis tables the views and the export read. It
//! lives under `internal:` beside the other alert state, and the map is
//! pruned every pass to the sessions that pass looked at, so it stays the
//! size of the activity window rather than the size of the archive.
//!
//! # What a notification carries
//!
//! A count, and nothing else. A session title comes from transcript text, so
//! naming the session here would put session content in a notification — see
//! the rules in [`crate::notifications`].

use std::collections::{BTreeMap, HashSet};

use tauri::AppHandle;

use crate::store::{SessionKey, Store};

/// Where the per-session counts survive a relaunch.
const SEEN_KEY: &str = "internal:cacheRehydrationSeen";

/// Whether `current` is worth announcing, given what the last pass recorded.
///
/// `seen` of `None` means no pass has looked at this session before. The
/// count becomes a baseline and nothing is announced.
pub fn is_new_rehydration(seen: Option<u64>, current: u64) -> bool {
    matches!(seen, Some(previous) if current > previous)
}

/// The identity a count is filed under. Includes the environment so the same
/// session id under two roots cannot share one baseline.
fn map_key(key: &SessionKey) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        key.environment_key, key.agent, key.session_id
    )
}

/// The per-session counts the last pass recorded.
///
/// A malformed or missing value reads as empty. The consequence is one quiet
/// pass while every session re-baselines, where the alternative — failing the
/// pass — would take scanning down over an alert's bookkeeping.
#[derive(Debug, Default)]
pub struct SeenCounts {
    counts: BTreeMap<String, u64>,
}

impl SeenCounts {
    pub fn load(store: &Store) -> Self {
        let counts = store
            .internal_value(SEEN_KEY)
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self { counts }
    }

    /// Record `current` for this session, and report whether it is a rise on
    /// what the last pass saw.
    pub fn observe(&mut self, key: &SessionKey, current: u64) -> bool {
        let key = map_key(key);
        let announce = is_new_rehydration(self.counts.get(&key).copied(), current);
        self.counts.insert(key, current);
        announce
    }

    /// Drop every session outside `keep`, so the map tracks the activity
    /// window rather than growing for the life of the install.
    pub fn retain(&mut self, keep: &HashSet<String>) {
        self.counts.retain(|key, _| keep.contains(key));
    }

    /// The key `retain` expects for a session this pass considered.
    pub fn retained_key(key: &SessionKey) -> String {
        map_key(key)
    }

    pub fn save(&self, store: &Store) {
        if let Ok(raw) = serde_json::to_string(&self.counts) {
            store.set_internal_value(SEEN_KEY, &raw);
        }
    }
}

/// Announce a rise in one session's rehydration count.
///
/// Split from [`SeenCounts::observe`] so a pass can decide and record without
/// a running application — the tests, and any caller with no `AppHandle`.
pub fn announce(app: &AppHandle, count: u64) {
    crate::notifications::note_cache_rehydration(app, count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_never_seen_before_only_baselines() {
        assert!(!is_new_rehydration(None, 0));
        assert!(
            !is_new_rehydration(None, 12),
            "a first sight of an old session must not announce its history"
        );
    }

    #[test]
    fn a_rise_announces_and_a_repeat_does_not() {
        assert!(is_new_rehydration(Some(0), 1));
        assert!(is_new_rehydration(Some(3), 9));
        assert!(!is_new_rehydration(Some(3), 3));
    }

    #[test]
    fn a_fall_does_not_announce() {
        // A re-analysis can report fewer turns than the last one — a shorter
        // window, or a transcript that was rewritten. That is not an event.
        assert!(!is_new_rehydration(Some(5), 2));
    }

    #[test]
    fn the_key_separates_environments() {
        let one = SessionKey::new("root-a", "claude", "s1");
        let two = SessionKey::new("root-b", "claude", "s1");
        assert_ne!(map_key(&one), map_key(&two));
    }

    #[test]
    fn observe_baselines_then_announces() {
        let mut seen = SeenCounts::default();
        let key = SessionKey::new("root", "claude", "s1");
        assert!(!seen.observe(&key, 2), "the first look is a baseline");
        assert!(!seen.observe(&key, 2), "an unchanged count says nothing");
        assert!(seen.observe(&key, 3), "one more rehydration is an event");
        assert!(!seen.observe(&key, 3));
    }

    #[test]
    fn retain_drops_sessions_outside_the_window() {
        let mut seen = SeenCounts::default();
        let kept = SessionKey::new("root", "claude", "kept");
        let dropped = SessionKey::new("root", "claude", "dropped");
        seen.observe(&kept, 1);
        seen.observe(&dropped, 1);

        let keep = HashSet::from([SeenCounts::retained_key(&kept)]);
        seen.retain(&keep);

        assert_eq!(seen.counts.len(), 1);
        assert!(seen.counts.contains_key(&map_key(&kept)));
    }

    #[test]
    fn the_state_key_is_internal() {
        assert!(SEEN_KEY.starts_with("internal:"));
    }
}
