// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local title generation for sessions stuck on the first-message fallback.
//!
//! The scan collects a [`LocalSummaryCandidate`] for each Codex session whose
//! title resolved to `firstMessage` and hands the batch to the title worker.
//! The worker owns all model time, so a degraded model can never block a
//! scan pass. Each accepted title lands with `localSummary` provenance. The
//! store write is guarded, so a vendor title or user rename that arrives in
//! the meantime wins.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use antiburn_local::titles::{
    SummarizerAvailability, TitleInput, TitleSummarizer, sanitize_generated_title,
};
use tauri::Manager;
use tokio::sync::Notify;

use crate::store::{SessionKey, Store};

/// One session that needs a generated title, with the bounded transcript
/// context the summarizer works from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalSummaryCandidate {
    pub key: SessionKey,
    pub input: TitleInput,
}

/// Wakes the title worker and carries the newest candidate batch.
///
/// The scan replaces the batch on every pass. A superseded batch loses
/// nothing: the next scan re-collects every session still on the
/// `firstMessage` fallback.
#[derive(Default)]
pub struct TitleWorkerHandle {
    wake: Notify,
    queue: Mutex<Vec<LocalSummaryCandidate>>,
}

/// Pause after a pass that produced nothing. A degraded model times out per
/// request, so an immediate retry would spend minutes for no titles.
const FAILURE_COOLDOWN_SECS: u64 = 600;

/// Hand `candidates` to the worker and wake it. The batch replaces any batch
/// the worker has not started yet.
pub fn enqueue(handle: &TitleWorkerHandle, candidates: Vec<LocalSummaryCandidate>) {
    if candidates.is_empty() {
        return;
    }
    if let Ok(mut queue) = handle.queue.lock() {
        *queue = candidates;
    }
    handle.wake.notify_one();
}

/// Run the title worker until the app exits.
pub fn spawn(app: &tauri::AppHandle) -> tauri::async_runtime::JoinHandle<()> {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let store = app.state::<Store>();
        let handle = app.state::<TitleWorkerHandle>();
        worker_loop(
            &store,
            &handle,
            platform_summarizer,
            Duration::from_secs(FAILURE_COOLDOWN_SECS),
        )
        .await;
    })
}

/// Process one batch per wake, newest batch only. After a failed pass the
/// worker sleeps for `failure_cooldown`; a batch that arrives in the
/// meantime is picked up when the cooldown ends.
pub(crate) async fn worker_loop(
    store: &Store,
    handle: &TitleWorkerHandle,
    summarizer: impl Fn() -> Option<Arc<dyn TitleSummarizer>>,
    failure_cooldown: Duration,
) {
    loop {
        handle.wake.notified().await;
        let candidates = match handle.queue.lock() {
            Ok(mut queue) => std::mem::take(&mut *queue),
            Err(_) => continue,
        };
        if candidates.is_empty() {
            continue;
        }
        let Some(backend) = summarizer() else {
            continue;
        };
        let stats = local_summary_pass(store, backend.as_ref(), &candidates).await;
        if stats.failed() {
            tokio::time::sleep(failure_cooldown).await;
        }
    }
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

/// What one pass did. The worker uses this for its cooldown decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassStats {
    /// False when the backend probe answered unavailable.
    pub available: bool,
    /// Sessions the pass asked the model to title.
    pub attempted: usize,
    /// Titles that reached the store.
    pub written: usize,
}

impl PassStats {
    /// True when the pass spent model or probe time and produced nothing.
    pub fn failed(&self) -> bool {
        !self.available || (self.attempted > 0 && self.written == 0)
    }
}

/// Generate and store titles for `candidates`. Returns what the pass did.
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
) -> PassStats {
    let mut stats = PassStats {
        available: true,
        attempted: 0,
        written: 0,
    };
    if candidates.is_empty() {
        return stats;
    }
    if let SummarizerAvailability::Unavailable(_) = summarizer.availability().await {
        stats.available = false;
        return stats;
    }
    for candidate in candidates {
        if stats.attempted >= MAX_TITLES_PER_PASS {
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
        stats.attempted += 1;
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
            stats.written += 1;
        }
    }
    stats
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
        store
            .upsert_sessions(&[seeded], &crate::agents::evidence_cohort())
            .unwrap();
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
        let stats = local_summary_pass(&store, &summarizer, &[candidate(&key)]).await;
        assert_eq!(stats.written, 1);
        assert!(!stats.failed());
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
        let off_stats = local_summary_pass(&store, &off, &[candidate(&key)]).await;
        assert_eq!(off_stats.written, 0);
        assert!(!off_stats.available);
        assert!(off_stats.failed());

        let refusing = FakeSummarizer::new(true, Some("I'm sorry, I cannot title this"));
        let refused_stats = local_summary_pass(&store, &refusing, &[candidate(&key)]).await;
        assert_eq!(refused_stats.written, 0);
        assert_eq!(refused_stats.attempted, 1);
        assert!(refused_stats.failed());
        let (_, source) = stored_title(&store, &key);
        assert_eq!(source.as_deref(), Some("firstMessage"));
    }

    #[tokio::test]
    async fn an_empty_or_generated_batch_is_not_a_failure() {
        let (store, key) = seeded_store("firstMessage");
        let summarizer = FakeSummarizer::new(true, Some("Make HUD sections clickable"));
        assert!(!local_summary_pass(&store, &summarizer, &[]).await.failed());

        // A batch whose sessions already carry a generated title costs no
        // model time, so it must not start a cooldown.
        local_summary_pass(&store, &summarizer, &[candidate(&key)]).await;
        let repeat = local_summary_pass(&store, &summarizer, &[candidate(&key)]).await;
        assert_eq!(repeat.attempted, 0);
        assert!(!repeat.failed());
    }

    #[tokio::test]
    async fn pass_generates_once_per_session() {
        let (store, key) = seeded_store("firstMessage");
        let summarizer = FakeSummarizer::new(true, Some("Make HUD sections clickable"));
        assert_eq!(
            local_summary_pass(&store, &summarizer, &[candidate(&key)])
                .await
                .written,
            1
        );
        // The next pass sees the same candidate; the model is not asked again.
        assert_eq!(
            local_summary_pass(&store, &summarizer, &[candidate(&key)])
                .await
                .written,
            0
        );
        assert_eq!(summarizer.calls(), 1);
    }

    #[tokio::test]
    async fn pass_never_overwrites_a_better_source() {
        let (store, key) = seeded_store("userRename");
        let summarizer = FakeSummarizer::new(true, Some("Generated title"));
        assert_eq!(
            local_summary_pass(&store, &summarizer, &[candidate(&key)])
                .await
                .written,
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
        store
            .upsert_sessions(&[rescan], &crate::agents::evidence_cohort())
            .unwrap();
        assert_eq!(
            stored_title(&store, &key),
            (
                Some("Make HUD sections clickable".into()),
                Some("localSummary".into())
            )
        );

        let renamed = record(&key, "Reader's own name", "userRename", 1_700_000_200);
        store
            .upsert_sessions(&[renamed], &crate::agents::evidence_cohort())
            .unwrap();
        let (title, source) = stored_title(&store, &key);
        assert_eq!(title.as_deref(), Some("Reader's own name"));
        assert_eq!(source.as_deref(), Some("userRename"));
    }

    /// A second session in the same store, so worker tests can send two
    /// distinct batches.
    fn seed_second(store: &Store) -> SessionKey {
        let key = SessionKey::new("native", "codex", "session-2");
        let seeded = record(&key, "please review this…", "firstMessage", 1_700_000_050);
        store
            .upsert_sessions(&[seeded], &crate::agents::evidence_cohort())
            .unwrap();
        key
    }

    async fn wait_until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !condition() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the worker reaches the expected state");
    }

    fn spawn_worker(
        store: &Arc<Store>,
        handle: &Arc<TitleWorkerHandle>,
        summarizer: &Arc<FakeSummarizer>,
        cooldown: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let store = Arc::clone(store);
        let handle = Arc::clone(handle);
        let summarizer = Arc::clone(summarizer);
        tokio::spawn(async move {
            worker_loop(
                &store,
                &handle,
                move || Some(Arc::clone(&summarizer) as Arc<dyn TitleSummarizer>),
                cooldown,
            )
            .await;
        })
    }

    #[test]
    fn enqueue_replaces_the_pending_batch_and_skips_empty_ones() {
        let handle = TitleWorkerHandle::default();
        let (store, first) = seeded_store("firstMessage");
        let second = seed_second(&store);
        enqueue(&handle, vec![candidate(&first)]);
        enqueue(&handle, vec![candidate(&second)]);
        let queue = handle.queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].key, second);
        drop(queue);

        let empty = TitleWorkerHandle::default();
        enqueue(&empty, vec![]);
        assert!(empty.queue.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn worker_cools_down_after_a_failed_pass() {
        let (store, first) = seeded_store("firstMessage");
        let store = Arc::new(store);
        let second = seed_second(&store);
        let handle = Arc::new(TitleWorkerHandle::default());
        // Every generation attempt fails, so each pass is a failure.
        let summarizer = Arc::new(FakeSummarizer::new(true, None));
        let worker = spawn_worker(
            &store,
            &handle,
            &summarizer,
            std::time::Duration::from_millis(150),
        );

        enqueue(&handle, vec![candidate(&first)]);
        wait_until(|| summarizer.calls() == 1).await;

        // The cooldown holds the next batch back.
        enqueue(&handle, vec![candidate(&second)]);
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert_eq!(summarizer.calls(), 1);

        // When the cooldown ends, the queued batch runs.
        wait_until(|| summarizer.calls() == 2).await;
        worker.abort();
    }

    #[tokio::test]
    async fn worker_takes_the_next_batch_at_once_after_a_successful_pass() {
        let (store, first) = seeded_store("firstMessage");
        let store = Arc::new(store);
        let second = seed_second(&store);
        let handle = Arc::new(TitleWorkerHandle::default());
        let summarizer = Arc::new(FakeSummarizer::new(
            true,
            Some("Make HUD sections clickable"),
        ));
        // The cooldown is far longer than the test; a successful pass must
        // not apply it.
        let worker = spawn_worker(
            &store,
            &handle,
            &summarizer,
            std::time::Duration::from_secs(3600),
        );

        enqueue(&handle, vec![candidate(&first)]);
        wait_until(|| summarizer.calls() == 1).await;
        enqueue(&handle, vec![candidate(&second)]);
        wait_until(|| summarizer.calls() == 2).await;
        let (_, source) = stored_title(&store, &second);
        assert_eq!(source.as_deref(), Some("localSummary"));
        worker.abort();
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
