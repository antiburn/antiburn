//! The daily `antiburn.claude_client_mix_observed` report.
//!
//! The report counts local Claude sessions active in the last
//! [`WINDOW_SECS`], by client label. It sends one event for each client with
//! a non-zero count. Each event carries the client label and a bucketed
//! count only. A durable marker in the settings table keeps the report to
//! one pass in [`MIN_INTERVAL_SECS`], across restarts.

use std::collections::BTreeMap;

use super::event::{self, EventName, Facts};
use crate::store::Store;

/// The `internal:` setting that holds the epoch of the last report.
const SENT_AT_KEY: &str = "internal:claudeClientMixSentAtEpochV1";

/// The minimum time between two reports.
const MIN_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// The activity window that the report counts.
const WINDOW_SECS: i64 = 7 * 24 * 60 * 60;

/// Map a stored client value to the closed vocabulary that may leave the
/// machine. A value outside the vocabulary becomes `unknown`.
fn client_label(stored: &str) -> &'static str {
    match stored {
        "cli" => "cli",
        "claude_desktop" => "claude_desktop",
        "vscode" => "vscode",
        "jetbrains" => "jetbrains",
        "sdk" => "sdk",
        _ => "unknown",
    }
}

fn sent_recently(store: &Store, now: i64) -> bool {
    store
        .internal_value(SENT_AT_KEY)
        .and_then(|value| value.parse::<i64>().ok())
        .is_some_and(|sent_at| (0..MIN_INTERVAL_SECS).contains(&now.saturating_sub(sent_at)))
}

/// Decide the events for one report and write the marker.
///
/// Returns nothing when `allowed` is `false`, when the last report is less
/// than [`MIN_INTERVAL_SECS`] old, or when no local Claude session is in the
/// window. Writes the marker before it returns events. If the marker write
/// fails, it returns nothing, so a failed write cannot cause a repeat report.
pub(super) fn reports(store: &Store, now: i64, allowed: bool) -> Vec<(EventName, Facts)> {
    if !allowed || sent_recently(store, now) {
        return Vec::new();
    }
    let Ok(rows) = store.claude_client_counts_since(now - WINDOW_SECS) else {
        return Vec::new();
    };
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    for (client, count) in rows {
        *counts.entry(client_label(&client)).or_default() += count;
    }
    counts.retain(|_, count| *count > 0);
    if counts.is_empty() {
        return Vec::new();
    }
    if store
        .set_internal_value_checked(SENT_AT_KEY, &now.to_string())
        .is_err()
    {
        return Vec::new();
    }
    counts
        .into_iter()
        .map(|(label, count)| {
            (
                EventName::ClaudeClientMixObserved,
                Facts {
                    label: Some(label),
                    bucket: Some(event::bucket(count)),
                    ..Facts::default()
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{SessionKey, SessionRecord};

    const NOW: i64 = 2_000_000_000;

    fn store() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory(directory.path()).unwrap();
        (directory, store)
    }

    fn session(id: &str, agent: &str, updated_at: i64, client: &str) -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", agent, id),
            source_kind: "file".into(),
            source_label: format!("/synthetic/{id}.jsonl"),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "cli".into(),
            client: client.into(),
            updated_at_epoch: Some(updated_at),
            activity_cursor: String::new(),
            activity_source: "event".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    fn seed(store: &Store, records: &[SessionRecord]) {
        store.upsert_sessions(records, &[]).unwrap();
    }

    fn sent(reports: &[(EventName, Facts)]) -> Vec<(&'static str, &'static str)> {
        reports
            .iter()
            .map(|(name, facts)| {
                assert_eq!(*name, EventName::ClaudeClientMixObserved);
                (facts.label.unwrap(), facts.bucket.unwrap())
            })
            .collect()
    }

    #[test]
    fn one_event_per_non_zero_client_with_a_bucketed_count() {
        let (_directory, store) = store();
        let mut records: Vec<_> = (0..12)
            .map(|index| session(&format!("cli-{index}"), "claude-code", NOW, "cli"))
            .collect();
        records.push(session("desktop", "claude-code", NOW, "claude_desktop"));
        records.push(session(
            "old",
            "claude-code",
            NOW - WINDOW_SECS - 1,
            "vscode",
        ));
        records.push(session("codex", "codex", NOW, "unknown"));
        seed(&store, &records);

        let reports = reports(&store, NOW, true);

        assert_eq!(
            sent(&reports),
            vec![("claude_desktop", "1-9"), ("cli", "10-49")]
        );
        for (_, facts) in &reports {
            assert!(facts.detail.is_none());
            assert!(facts.origin.is_none());
            assert!(facts.plan.is_none());
            assert!(facts.unrecognized_types.is_none());
        }
    }

    #[test]
    fn a_stored_value_outside_the_vocabulary_is_sent_as_unknown() {
        let (_directory, store) = store();
        seed(
            &store,
            &[
                session("raw", "claude-code", NOW, "claude-vscode-raw"),
                session("legacy", "claude-code", NOW, "unknown"),
            ],
        );

        assert_eq!(sent(&reports(&store, NOW, true)), vec![("unknown", "1-9")]);
    }

    #[test]
    fn no_claude_sessions_send_nothing_and_leave_no_marker() {
        let (_directory, store) = store();
        seed(&store, &[session("codex", "codex", NOW, "unknown")]);

        assert!(reports(&store, NOW, true).is_empty());
        assert!(store.internal_value(SENT_AT_KEY).is_none());
    }

    #[test]
    fn analytics_off_sends_nothing_and_leaves_no_marker() {
        let (_directory, store) = store();
        seed(&store, &[session("s", "claude-code", NOW, "cli")]);

        assert!(reports(&store, NOW, false).is_empty());
        assert!(store.internal_value(SENT_AT_KEY).is_none());
        assert_eq!(reports(&store, NOW, true).len(), 1);
    }

    #[test]
    fn the_durable_marker_limits_reports_to_one_a_day() {
        let (_directory, store) = store();
        seed(&store, &[session("s", "claude-code", NOW, "cli")]);

        assert_eq!(reports(&store, NOW, true).len(), 1);
        assert!(reports(&store, NOW + 1, true).is_empty());
        assert!(reports(&store, NOW + MIN_INTERVAL_SECS - 1, true).is_empty());
        assert_eq!(reports(&store, NOW + MIN_INTERVAL_SECS, true).len(), 1);
    }

    #[test]
    fn the_marker_survives_a_reopened_store() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = Store::open(directory.path()).unwrap();
            seed(&store, &[session("s", "claude-code", NOW, "cli")]);
            assert_eq!(reports(&store, NOW, true).len(), 1);
        }
        let store = Store::open(directory.path()).unwrap();
        assert!(reports(&store, NOW + 60, true).is_empty());
    }
}
