//! Phase 3b integration coverage: the durable worker resumes a Claude file
//! source from a stored snapshot instead of reading it in full every pass.
//!
//! Every test here drives the real worker path (`run_record_pass_with` and
//! `process_next`, with a real [`Store`]) over transcript files on disk, the
//! same shape `pi_file_flows_through_worker_persistence_and_report` above
//! uses. Only the transcript's own location is synthetic: a custom
//! `RecordAnalyzer` points `evidence_pass_with_turn_rows` at a temp file
//! instead of a discovered one, so the test avoids mutating process-global
//! `$HOME` state to exercise real filesystem discovery.

use std::collections::HashMap;

use antiburn_local::analysis::StreamSnapshot;

use super::*;
use crate::store::{SourcePublishMode, SourcePublishOutcome};

/// One synthetic Claude assistant record. `salt` lets two records at the
/// same `index` differ in their bytes — used to simulate a rewritten line.
fn assistant_line(label: &str, index: usize, salt: u64) -> String {
    let model = if index.is_multiple_of(3) {
        "claude-opus-4-6"
    } else {
        "claude-sonnet-4-6"
    };
    format!(
        "{{\"type\":\"assistant\",\"timestamp\":{ts},\"message\":{{\"id\":\"m-{label}-{index}-{salt}\",\"role\":\"assistant\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":{inp},\"output_tokens\":{outp}}},\"content\":[{{\"type\":\"text\",\"text\":\"reply {index} for {label}\"}}]}}}}\n",
        ts = 1_700_000_000i64 + (index as i64) * 5,
        inp = 10 + index as u64,
        outp = 5 + index as u64,
    )
}

fn lines(label: &str, range: std::ops::Range<usize>, salt: u64) -> String {
    range
        .map(|index| assistant_line(label, index, salt))
        .collect()
}

/// The parent transcript's content at each of the four growth steps.
/// Step three (index 2) rewrites record 5 — already published at step two
/// — before growing further, so a resumed pass must detect the change
/// instead of trusting its stored tail hash.
fn parent_content(step: usize) -> String {
    match step {
        0 => lines("parent", 0..3, 0),
        1 => lines("parent", 0..6, 0),
        2 => lines("parent", 0..5, 0) + &assistant_line("parent", 5, 1) + &lines("parent", 6..9, 0),
        3 => {
            lines("parent", 0..5, 0) + &assistant_line("parent", 5, 1) + &lines("parent", 6..12, 0)
        }
        _ => unreachable!("only four growth steps are defined"),
    }
}

/// The child (sub-agent) transcript's content at each step. It has no file
/// at all at step zero — it appears at step one and grows again at step two.
fn child_content(step: usize) -> Option<String> {
    match step {
        0 => None,
        1 => Some(lines("child", 0..3, 0)),
        2 => Some(lines("child", 0..6, 0)),
        3 => Some(lines("child", 0..8, 0)),
        _ => unreachable!("only four growth steps are defined"),
    }
}

/// Drives one worker pass over `parent_path` (and `child_path`, when a
/// child is present at this step) through the real `process_next` /
/// `run_record_pass_with` path, with a custom analyzer standing in for
/// filesystem discovery. Returns the mode (`Full` or `Resumed`) this pass
/// recorded for each source it visited, captured from the pass's own
/// `source_outcomes` before `process_next` consumes it — this is the only
/// direct evidence a pass actually resumed, rather than silently falling
/// back to a full read that happens to agree with one.
async fn run_worker_step(
    store: &Store,
    id: &str,
    now: i64,
    first_step: bool,
    parent_path: &std::path::Path,
    child_path: Option<&std::path::Path>,
) -> HashMap<String, SourcePublishMode> {
    if first_step {
        store
            .upsert_sessions(&[record(id)], &crate::agents::evidence_cohort())
            .unwrap();
    } else {
        store.requeue_session_evidence(&record(id).key).unwrap();
    }
    let parent_path = parent_path.to_path_buf();
    let child_path = child_path.map(std::path::Path::to_path_buf);
    let captured_outcomes: Arc<Mutex<Vec<SourcePublishOutcome>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_outcomes_for_analyzer = Arc::clone(&captured_outcomes);
    let analyzer = move |_agent: AgentKind,
                         session_id: String,
                         _wsl_distro: Option<String>,
                         _claimed: analysis::ClaimedSource,
                         signal: PassSignal,
                         turn_row_store: Option<Arc<dyn TurnRowStore>>| {
        let child_session_id = format!("{session_id}-child");
        let mut inputs = vec![SessionInput {
            agent: "claude".into(),
            session_id,
            source: RawSource::File(parent_path.clone()),
        }];
        if let Some(child_path) = child_path.clone() {
            inputs.push(SessionInput {
                agent: "claude".into(),
                session_id: child_session_id,
                source: RawSource::File(child_path),
            });
        }
        let captured_outcomes = Arc::clone(&captured_outcomes_for_analyzer);
        Box::pin(async move {
            let pass = analysis::evidence_pass_with_turn_rows(
                &inputs,
                &|| signal.observe(),
                turn_row_store,
            );
            *captured_outcomes.lock().unwrap() = pass.source_outcomes.clone();
            pass
        }) as PassFuture
    };
    let store_for_runner = store.clone();
    let runner = move |record: &SessionRecord, signal: PassSignal, claim_fence: i64| {
        run_record_pass_with(
            record,
            signal,
            claim_fence,
            store_for_runner.clone(),
            &analyzer,
        )
    };
    assert!(
        process_next(store, &WorkerHandle::default(), &|| now, &runner, &|_| {})
            .await
            .unwrap(),
        "the worker must claim and publish this step"
    );
    captured_outcomes
        .lock()
        .unwrap()
        .iter()
        .map(|outcome| (outcome.source_key.clone(), outcome.mode))
        .collect()
}

/// A fresh store, given exactly one pass over `parent_path` (and
/// `child_path`, if present) — the "analysed by a fresh full pass"
/// reference this module's tests compare a resumed store against.
async fn fresh_full_read(
    id: &str,
    now: i64,
    parent_path: &std::path::Path,
    child_path: Option<&std::path::Path>,
) -> Store {
    let reference = Store::open_in_memory(std::path::Path::new(
        "/tmp/antiburn-worker-resume-reference",
    ))
    .unwrap();
    run_worker_step(&reference, id, now, true, parent_path, child_path).await;
    reference
}

/// Asserts every projection `publish_projections` writes agrees between
/// `resumed` and `reference` for session `id`: published turn rows, the
/// published coverage record, the `session_analysis` columns, and the
/// evidence JSON. `context` names the comparison in a failed assertion.
fn assert_projections_match(resumed: &Store, reference: &Store, id: &str, context: &str) {
    let key = record(id).key;

    let mut resumed_rows = resumed.published_turn_rows(&key).unwrap().unwrap();
    let mut reference_rows = reference.published_turn_rows(&key).unwrap().unwrap();
    resumed_rows.sort_by_key(|row| (row.source_key.clone(), row.turn_index));
    reference_rows.sort_by_key(|row| (row.source_key.clone(), row.turn_index));
    assert_eq!(resumed_rows, reference_rows, "turn rows differ ({context})");

    assert_eq!(
        resumed.published_coverage_record(&key).unwrap(),
        reference.published_coverage_record(&key).unwrap(),
        "coverage record differs ({context})"
    );

    assert_eq!(
        resumed.analysis(&key).unwrap().unwrap(),
        reference.analysis(&key).unwrap().unwrap(),
        "session_analysis differs ({context})"
    );

    let resumed_evidence = resumed
        .evidence(&key)
        .unwrap()
        .unwrap()
        .evidence_json
        .unwrap();
    let reference_evidence = reference
        .evidence(&key)
        .unwrap()
        .unwrap()
        .evidence_json
        .unwrap();
    let resumed_evidence: serde_json::Value = serde_json::from_str(&resumed_evidence).unwrap();
    let reference_evidence: serde_json::Value = serde_json::from_str(&reference_evidence).unwrap();
    assert_eq!(
        resumed_evidence, reference_evidence,
        "evidence JSON differs ({context})"
    );
}

/// The main resume scenario: a parent transcript grows over four steps,
/// with a child (sub-agent) transcript appearing at step two and growing
/// again at step three; step three also rewrites a record the previous
/// step already published, forcing that source back to a full read. After
/// every step, the resumed store's published projections must equal a
/// fresh store's single full pass over the same content — and, since that
/// comparison alone would also pass if every step silently fell back to a
/// full read, this also asserts the pass's own recorded mode for each
/// source, and that a source with a stored resume has an offset tracking
/// its file's length, so a genuine resume is the thing actually under
/// test.
#[tokio::test]
async fn a_growing_transcript_with_a_child_matches_fresh_full_reads_at_every_step() {
    let tmp = tempfile::tempdir().unwrap();
    let parent_path = tmp.path().join("parent.jsonl");
    let child_path = tmp.path().join("child.jsonl");
    let id = "growing-parent";
    let child_source_key = format!("{id}-child");
    let key = record(id).key;
    let store = store();
    let base_now = 1_770_200_000;

    // Full at step 0 (nothing to resume from yet) and step 2 (the tail
    // rewrite forces a full re-read); Resumed at every other step,
    // including step 3. The retry that follows a rewrite still bootstraps
    // through `visit_claimed_resumed` (see `delete_then_full_read`), so it
    // stores a fresh resume just like the very first pass would, and the
    // next pass can resume from it normally.
    let expected_parent_mode = [
        SourcePublishMode::Full,
        SourcePublishMode::Resumed,
        SourcePublishMode::Full,
        SourcePublishMode::Resumed,
    ];
    // Absent before the child exists; Full the step it first appears;
    // Resumed once it has grown with no rewrite of its own.
    let expected_child_mode = [
        None,
        Some(SourcePublishMode::Full),
        Some(SourcePublishMode::Resumed),
        Some(SourcePublishMode::Resumed),
    ];

    for step in 0..4 {
        std::fs::write(&parent_path, parent_content(step)).unwrap();
        let child = child_content(step);
        if let Some(content) = &child {
            std::fs::write(&child_path, content).unwrap();
        }
        let child_ref = child.as_ref().map(|_| child_path.as_path());

        let modes = run_worker_step(
            &store,
            id,
            base_now + step as i64,
            step == 0,
            &parent_path,
            child_ref,
        )
        .await;
        assert_eq!(
            modes.get(id).copied(),
            Some(expected_parent_mode[step]),
            "parent mode at step {step}"
        );
        assert_eq!(
            modes.get(&child_source_key).copied(),
            expected_child_mode[step],
            "child mode at step {step}"
        );

        let evidence = store.evidence(&key).unwrap().unwrap();
        let fenced: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
            store.clone(),
            key.clone(),
            evidence.claim_fence,
        ));
        // Every step, including the rewrite step, must leave the parent
        // resumable: its stored snapshot decodes and its offset tracks the
        // (possibly just-rewritten) file's current length.
        let stored = fenced.read_resume(id).unwrap().unwrap_or_else(|| {
            panic!("a published pass must store the parent's resume at step {step}")
        });
        let snapshot =
            StreamSnapshot::decode(&stored.snapshot).expect("the stored snapshot must decode");
        let parent_len = std::fs::metadata(&parent_path).unwrap().len();
        assert_eq!(
            snapshot.resume.offset, parent_len,
            "the parent's stored resume offset must equal the file's length at step {step}"
        );

        let reference = fresh_full_read(id, base_now + step as i64, &parent_path, child_ref).await;

        assert_projections_match(&store, &reference, id, &format!("step {step}"));
    }
}

/// A resume snapshot stored under a revision the worker no longer
/// recognizes (simulating a leftover row from before a revision bump) must
/// be ignored, not restored: the next pass falls back to a full bootstrap
/// read and stores a fresh, current snapshot in its place.
#[tokio::test]
async fn a_stale_snapshot_revision_row_is_ignored_and_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let parent_path = tmp.path().join("parent.jsonl");
    let id = "stale-resume";
    let store = store();
    let key = record(id).key;

    std::fs::write(&parent_path, lines("stale", 0..3, 0)).unwrap();
    run_worker_step(&store, id, 1_770_300_000, true, &parent_path, None).await;

    let evidence = store.evidence(&key).unwrap().unwrap();
    let fenced: Arc<dyn TurnRowStore> = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        key.clone(),
        evidence.claim_fence,
    ));
    let mut stale = fenced
        .read_resume(id)
        .unwrap()
        .expect("a published pass must store a resume snapshot");
    stale.snapshot_revision -= 1;
    fenced.write_resume(id, stale).unwrap();

    std::fs::write(&parent_path, lines("stale", 0..6, 0)).unwrap();
    run_worker_step(&store, id, 1_770_300_001, false, &parent_path, None).await;

    let reference = fresh_full_read(id, 1_770_300_001, &parent_path, None).await;
    assert_projections_match(&store, &reference, id, "after a stale resume row");

    let refreshed = fenced
        .read_resume(id)
        .unwrap()
        .expect("the stale row must be replaced with a fresh one, not left in place");
    assert_eq!(
        refreshed.snapshot_revision,
        analysis::resume_revisions().snapshot_revision
    );
}
