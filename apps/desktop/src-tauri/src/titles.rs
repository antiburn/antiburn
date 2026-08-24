// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local title generation for sessions stuck on the first-message fallback.
//!
//! The scan collects a [`LocalSummaryCandidate`] for each Codex session whose
//! title resolved to `firstMessage`. After the pass persists its records, the
//! shell hands the candidates to the platform summarizer, and each accepted
//! title lands with `localSummary` provenance. The store write is guarded, so
//! a vendor title or user rename that arrives in the meantime wins.

use std::sync::Arc;

use antiburn_local::titles::{
    SummarizerAvailability, TitleInput, TitleSummarizer, sanitize_generated_title,
};

use crate::store::{SessionKey, Store};

/// One session that needs a generated title, with the bounded transcript
/// context the summarizer works from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSummaryCandidate {
    pub key: SessionKey,
    pub input: TitleInput,
}

/// The summarizer this platform ships, when one exists.
///
/// No backend ships yet. Step 3 of `docs/plans/local-session-titles.md`
/// returns the macOS Foundation Models sidecar here; other platforms stay
/// `None` and keep the cleaned first-message fallback.
pub fn platform_summarizer() -> Option<Arc<dyn TitleSummarizer>> {
    None
}

/// Generate and store titles for `candidates`. Returns how many rows changed.
///
/// Availability is probed once per pass, not cached across passes — the user
/// can turn the underlying model off at any time. Failures are silent by
/// design: the fallback title is already on screen, and a missed candidate is
/// retried when a later scan sees the session still on `firstMessage`.
pub async fn local_summary_pass(
    store: &Store,
    summarizer: &dyn TitleSummarizer,
    candidates: &[LocalSummaryCandidate],
) -> usize {
    if candidates.is_empty() {
        return 0;
    }
    if let SummarizerAvailability::Unavailable(_) = summarizer.availability().await {
        return 0;
    }
    let mut written = 0;
    for candidate in candidates {
        let Some(raw) = summarizer.title(&candidate.input).await else {
            continue;
        };
        let Some(title) = sanitize_generated_title(&raw) else {
            continue;
        };
        if store
            .set_local_summary_title(&candidate.key, &title)
            .unwrap_or(false)
        {
            written += 1;
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::store::SessionRecord;
    use async_trait::async_trait;

    struct FakeSummarizer {
        available: bool,
        reply: Option<&'static str>,
    }

    #[async_trait]
    impl TitleSummarizer for FakeSummarizer {
        async fn availability(&self) -> SummarizerAvailability {
            if self.available {
                SummarizerAvailability::Available
            } else {
                SummarizerAvailability::Unavailable("test backend off".into())
            }
        }

        async fn title(&self, _input: &TitleInput) -> Option<String> {
            self.reply.map(str::to_string)
        }
    }

    fn record(key: &SessionKey, title: &str, title_source: &str, updated_at: i64) -> SessionRecord {
        SessionRecord {
            key: key.clone(),
            source_kind: "file".into(),
            source_label: "rollout.jsonl".into(),
            wsl_distro: None,
            title: Some(title.into()),
            title_source: Some(title_source.into()),
            cwd: Some("/repo/antiburn".into()),
            surface: "cli".into(),
            updated_at_epoch: Some(updated_at),
            activity_cursor: String::new(),
            activity_source: "mtime".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: None,
        }
    }

    fn seeded_store(title_source: &str) -> (Store, SessionKey) {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-test-state")).unwrap();
        let key = SessionKey::new("native", "codex", "session-1");
        let seeded = record(
            &key,
            "in this pane, it should be possible…",
            title_source,
            1_700_000_000,
        );
        store.upsert_sessions(&[seeded]).unwrap();
        (store, key)
    }

    fn candidate(key: &SessionKey) -> LocalSummaryCandidate {
        LocalSummaryCandidate {
            key: key.clone(),
            input: TitleInput {
                repo: Some("antiburn".into()),
                first_message: "in this pane, it should be possible to click".into(),
                context: vec![],
            },
        }
    }

    fn stored_title(store: &Store, key: &SessionKey) -> (Option<String>, Option<String>) {
        let sessions = store.recent_sessions(0, 10).unwrap();
        let session = sessions
            .into_iter()
            .find(|record| record.key == *key)
            .unwrap();
        (session.title, session.title_source)
    }

    #[tokio::test]
    async fn pass_writes_sanitized_title_with_local_summary_provenance() {
        let (store, key) = seeded_store("firstMessage");
        let summarizer = FakeSummarizer {
            available: true,
            reply: Some("\"Make HUD sections clickable.\""),
        };
        let written = local_summary_pass(&store, &summarizer, &[candidate(&key)]).await;
        assert_eq!(written, 1);
        assert_eq!(
            stored_title(&store, &key),
            (
                Some("Make HUD sections clickable".into()),
                Some("localSummary".into())
            )
        );
    }

    #[tokio::test]
    async fn pass_skips_unavailable_backend_and_refusals() {
        let (store, key) = seeded_store("firstMessage");
        let off = FakeSummarizer {
            available: false,
            reply: Some("Never used"),
        };
        assert_eq!(
            local_summary_pass(&store, &off, &[candidate(&key)]).await,
            0
        );

        let refusing = FakeSummarizer {
            available: true,
            reply: Some("I'm sorry, I cannot title this"),
        };
        assert_eq!(
            local_summary_pass(&store, &refusing, &[candidate(&key)]).await,
            0
        );
        let (_, source) = stored_title(&store, &key);
        assert_eq!(source.as_deref(), Some("firstMessage"));
    }

    #[tokio::test]
    async fn pass_never_overwrites_a_better_source() {
        let (store, key) = seeded_store("userRename");
        let summarizer = FakeSummarizer {
            available: true,
            reply: Some("Generated title"),
        };
        assert_eq!(
            local_summary_pass(&store, &summarizer, &[candidate(&key)]).await,
            0
        );
        let (title, source) = stored_title(&store, &key);
        assert_eq!(source.as_deref(), Some("userRename"));
        assert_eq!(
            title.as_deref(),
            Some("in this pane, it should be possible…")
        );
    }

    #[tokio::test]
    async fn rescan_keeps_local_summary_over_first_message() {
        let (store, key) = seeded_store("firstMessage");
        store
            .set_local_summary_title(&key, "Make HUD sections clickable")
            .unwrap();

        // A later scan re-upserts the same first-message fallback; the
        // generated title must survive it.
        let rescan = record(
            &key,
            "in this pane, it should be possible…",
            "firstMessage",
            1_700_000_100,
        );
        store.upsert_sessions(&[rescan]).unwrap();
        assert_eq!(
            stored_title(&store, &key),
            (
                Some("Make HUD sections clickable".into()),
                Some("localSummary".into())
            )
        );

        let renamed = record(&key, "Reader's own name", "userRename", 1_700_000_200);
        store.upsert_sessions(&[renamed]).unwrap();
        let (title, source) = stored_title(&store, &key);
        assert_eq!(title.as_deref(), Some("Reader's own name"));
        assert_eq!(source.as_deref(), Some("userRename"));
    }
}
