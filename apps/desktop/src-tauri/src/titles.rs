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
/// macOS ships the Foundation Models sidecar. Other platforms return `None`
/// and keep the cleaned first-message fallback.
#[cfg(target_os = "macos")]
pub fn platform_summarizer() -> Option<Arc<dyn TitleSummarizer>> {
    let binary = sidecar::binary_path()?;
    Some(Arc::new(sidecar::SidecarSummarizer::new(binary)))
}

#[cfg(not(target_os = "macos"))]
pub fn platform_summarizer() -> Option<Arc<dyn TitleSummarizer>> {
    None
}

/// The macOS backend: a bundled Swift helper (`run-foundation-model`) that
/// talks to the on-device Apple Foundation Models. The helper is a generic
/// prompt runner; this module builds the title-specific request. One process
/// run per request: `--probe` answers availability, and a run without
/// arguments reads `{"instructions", "prompt"}` JSON on stdin and writes the
/// model response to stdout.
#[cfg(target_os = "macos")]
mod sidecar {
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;

    use antiburn_local::titles::{
        SummarizerAvailability, TITLE_INSTRUCTIONS, TitleInput, TitleSummarizer, title_prompt,
    };
    use async_trait::async_trait;
    use tokio::io::AsyncWriteExt;

    /// Wall time for one helper run. Generation takes 300ms–1s warm; the
    /// margin covers a cold model load. On timeout the process is killed and
    /// the session keeps its fallback title.
    const RUN_TIMEOUT: Duration = Duration::from_secs(20);

    pub struct SidecarSummarizer {
        binary: PathBuf,
    }

    impl SidecarSummarizer {
        pub fn new(binary: PathBuf) -> Self {
            Self { binary }
        }

        async fn run(&self, args: &[&str], stdin: Option<Vec<u8>>) -> Option<String> {
            let mut command = tokio::process::Command::new(&self.binary);
            command
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let mut child = command.spawn().ok()?;
            if let Some(payload) = stdin
                && let Some(mut input) = child.stdin.take()
            {
                let _ = input.write_all(&payload).await;
            } else {
                drop(child.stdin.take());
            }
            let output = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
                .await
                .ok()?
                .ok()?;
            if !output.status.success() {
                return None;
            }
            String::from_utf8(output.stdout).ok()
        }
    }

    #[async_trait]
    impl TitleSummarizer for SidecarSummarizer {
        async fn availability(&self) -> SummarizerAvailability {
            match self.run(&["--probe"], None).await {
                Some(answer) if answer.trim() == "available" => SummarizerAvailability::Available,
                Some(answer) => SummarizerAvailability::Unavailable(answer.trim().to_string()),
                None => SummarizerAvailability::Unavailable("the sidecar did not answer".into()),
            }
        }

        async fn title(&self, input: &TitleInput) -> Option<String> {
            let request = serde_json::json!({
                "instructions": TITLE_INSTRUCTIONS,
                "prompt": title_prompt(input),
            });
            let payload = serde_json::to_vec(&request).ok()?;
            self.run(&[], Some(payload)).await
        }
    }

    /// Find the bundled helper. A packaged app carries it beside the main
    /// executable; a development build falls back to the copy that build.rs
    /// compiled into the manifest's `binaries/` directory.
    pub fn binary_path() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bundled = exe.parent()?.join("run-foundation-model");
        if bundled.is_file() {
            return Some(bundled);
        }
        if cfg!(debug_assertions) {
            let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join(format!(
                    "run-foundation-model-{}",
                    env!("ANTIBURN_TARGET_TRIPLE")
                ));
            if dev.is_file() {
                return Some(dev);
            }
        }
        None
    }
}

/// Model calls one pass may spend. A first scan can surface hundreds of
/// candidates at once; the rest catch up on later passes, newest first.
const MAX_TITLES_PER_PASS: usize = 15;

/// Generate and store titles for `candidates`. Returns how many rows changed.
///
/// Availability is probed once per pass, not cached across passes — the user
/// can turn the underlying model off at any time. A session already on
/// `localSummary` is skipped: one generated title per session, then done.
/// Failures are silent by design: the fallback title is already on screen,
/// and a missed candidate is retried when a later scan sees the session
/// still on `firstMessage`.
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
    let mut generated = 0;
    for candidate in candidates {
        if generated >= MAX_TITLES_PER_PASS {
            break;
        }
        let already_generated = store
            .session(&candidate.key)
            .ok()
            .flatten()
            .is_some_and(|session| session.title_source.as_deref() == Some("localSummary"));
        if already_generated {
            continue;
        }
        generated += 1;
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
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeSummarizer {
        fn new(available: bool, reply: Option<&'static str>) -> Self {
            Self {
                available,
                reply,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
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
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        let summarizer = FakeSummarizer::new(true, Some("\"Make HUD sections clickable.\""));
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
        let off = FakeSummarizer::new(false, Some("Never used"));
        assert_eq!(
            local_summary_pass(&store, &off, &[candidate(&key)]).await,
            0
        );

        let refusing = FakeSummarizer::new(true, Some("I'm sorry, I cannot title this"));
        assert_eq!(
            local_summary_pass(&store, &refusing, &[candidate(&key)]).await,
            0
        );
        let (_, source) = stored_title(&store, &key);
        assert_eq!(source.as_deref(), Some("firstMessage"));
    }

    #[tokio::test]
    async fn pass_generates_once_per_session() {
        let (store, key) = seeded_store("firstMessage");
        let summarizer = FakeSummarizer::new(true, Some("Make HUD sections clickable"));
        assert_eq!(
            local_summary_pass(&store, &summarizer, &[candidate(&key)]).await,
            1
        );
        // The next pass sees the same candidate; the model is not asked again.
        assert_eq!(
            local_summary_pass(&store, &summarizer, &[candidate(&key)]).await,
            0
        );
        assert_eq!(summarizer.calls(), 1);
    }

    #[tokio::test]
    async fn pass_never_overwrites_a_better_source() {
        let (store, key) = seeded_store("userRename");
        let summarizer = FakeSummarizer::new(true, Some("Generated title"));
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

    #[cfg(target_os = "macos")]
    mod sidecar {
        use super::super::sidecar::SidecarSummarizer;
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// A stand-in helper script, so the tests cover the process protocol
        /// without the real model.
        fn stub(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
            let path = dir.join("run-foundation-model");
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        #[tokio::test]
        async fn probe_maps_stdout_to_availability() {
            let dir = tempfile::tempdir().unwrap();
            let available = SidecarSummarizer::new(stub(dir.path(), r#"echo "available""#));
            assert_eq!(
                available.availability().await,
                SummarizerAvailability::Available
            );

            let off = SidecarSummarizer::new(stub(
                dir.path(),
                r#"echo "unavailable: Apple Intelligence is off""#,
            ));
            assert_eq!(
                off.availability().await,
                SummarizerAvailability::Unavailable(
                    "unavailable: Apple Intelligence is off".into()
                )
            );
        }

        #[tokio::test]
        async fn title_sends_instructions_and_prompt_json() {
            let dir = tempfile::tempdir().unwrap();
            // The stub echoes the JSON it receives, so the assertion covers
            // both directions of the protocol.
            let echoing = SidecarSummarizer::new(stub(dir.path(), "cat"));
            let input = TitleInput {
                repo: Some("antiburn".into()),
                first_message: "make the pane clickable".into(),
                context: vec!["also fix hover".into()],
            };
            let round_trip = echoing.title(&input).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&round_trip).unwrap();
            assert_eq!(
                request["instructions"],
                antiburn_local::titles::TITLE_INSTRUCTIONS
            );
            assert_eq!(
                request["prompt"],
                "Repository: antiburn\nFirst message: make the pane clickable\nLater messages:\n- also fix hover"
            );
        }

        #[tokio::test]
        async fn a_failing_helper_yields_no_title() {
            let dir = tempfile::tempdir().unwrap();
            let failing = SidecarSummarizer::new(stub(dir.path(), "exit 1"));
            let input = TitleInput {
                repo: None,
                first_message: "anything".into(),
                context: vec![],
            };
            assert!(failing.title(&input).await.is_none());
            assert!(matches!(
                failing.availability().await,
                SummarizerAvailability::Unavailable(_)
            ));
        }
    }
}
