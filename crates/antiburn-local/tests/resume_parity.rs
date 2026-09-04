//! Parity check for verified-resume ingest (continuous ingest, phase 3a).
//!
//! A file grows in record-aligned steps `S0 ⊂ S1 ⊂ ... ⊂ Sn`. "Resumed at
//! step k" — a full pass over `S0` with a snapshot, then `visit_claimed_resumed`
//! once per later step through `k`, each time restoring the metrics and
//! evidence accumulators and the row sink's next index from the previous
//! step's snapshot — must equal "a full pass at `Sk`" for turn rows (every
//! column, plus their `turn_content`), `SessionMetrics`, `SessionSummary`,
//! `TurnFacts`, and `SessionEvidence`. See "Why a snapshot and not an
//! offset" in `docs/plans/continuous-session-ingest.md`.
//!
//! [`assert_resume_parity`] is the driver every positive test below calls.
//! Every compared value — rows, turn content, `TurnFacts`, `SessionMetrics`
//! (including `buckets`, the chart data), `SessionSummary`, and
//! `SessionEvidence` — is asserted exactly.
//!
//! The negative tests at the bottom exercise the three ways a resumed pass
//! must refuse instead of silently reusing stale state: a rewritten tail, a
//! truncation below the resume offset, and a stale snapshot revision.

#[path = "support/claude_fixture.rs"]
mod claude_fixture;
#[path = "support/corpus.rs"]
mod corpus;

use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::{
    AppendOnlyGuarantee, ClaudeAdapter, CompositeSink, EvidenceSnapshot, EvidenceSource,
    FenceScope, MemoryTurnRowStore, RESUME_SNAPSHOT_REVISION, RawSource, ResumePoint,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceCapabilities,
    SourceChangedReason, SourceClaim, SourceKind, StreamSnapshot, TurnRowSink, TurnRowStore,
    TurnSessionKey, VisitOutcome, adapter_for, query_turn_facts, query_turn_rows,
};
use antiburn_local::discovery::source_version::head_hash_of;
use antiburn_local::discovery::{FingerprintInputs, SourceStat};
use corpus::{SessionSpec, generate_session};
use tempfile::TempDir;

/* --------------------------------------------------------------------
 * Helpers shared by every test below.
 * ----------------------------------------------------------------- */

/// A claim covering the file's current full content: the shape both
/// `visit_claimed` and `visit_claimed_resumed` expect from a fresh open.
fn claim_for(path: &Path) -> SourceClaim {
    let file = std::fs::File::open(path).expect("open source for claim");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source for claim");
    let bytes = std::fs::read(path).expect("read source for claim");
    SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&bytes)),
    })
}

/// A [`StreamSnapshot`] ready to start the very first resumable pass over a
/// source: [`ResumePoint`] offset zero (an empty tail, so
/// `PinnedSource::open_resumed` resumes as a full read) and `agent`'s own
/// fresh adapter state.
fn fresh_snapshot(
    agent: &str,
    session_id: &str,
    capabilities: SourceCapabilities,
) -> StreamSnapshot {
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        kind: SourceKind::Jsonl,
        capabilities,
    });
    StreamSnapshot {
        revision: RESUME_SNAPSHOT_REVISION,
        resume: ResumePoint {
            offset: 0,
            tail_hash: head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: adapter_for(agent)
            .empty_resume_state()
            .expect("adapter under test must support resume"),
        metrics: SessionMetricsAccumulator::new(agent, session_id),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: Default::default(),
        },
        next_turn_index: 0,
    }
}

fn session_input(agent: &str, session_id: &str, path: &Path) -> SessionInput {
    SessionInput {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::File(path.to_path_buf()),
        fork_parent_session_id: None,
    }
}

/// Splits `jsonl` (one JSON record per line) into `num_steps` growing,
/// record-aligned prefixes, the last of which is the whole input.
/// `num_steps` must not exceed the record count.
fn record_aligned_steps(jsonl: &str, num_steps: usize) -> Vec<String> {
    let lines: Vec<&str> = jsonl.lines().collect();
    assert!(
        lines.len() >= num_steps,
        "{} records cannot cut into {num_steps} steps",
        lines.len()
    );
    (1..=num_steps)
        .map(|step| {
            let cut = (lines.len() * step) / num_steps;
            let mut prefix = lines[..cut].join("\n");
            prefix.push('\n');
            prefix
        })
        .collect()
}

/// Every `turn_content` row for `key` at `claim_fence`, ordered the same way
/// `query_turn_rows` orders its rows. `query_turn_rows` never joins this
/// table (its `TurnRow::content` is always empty), so this is the separate
/// content read the module doc promises.
fn turn_content_rows(
    store: &MemoryTurnRowStore,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> Vec<(String, u64, i64, String, Vec<u8>, bool)> {
    store.with_connection(|conn| {
        let mut statement = conn
            .prepare(
                "SELECT t.source_key, t.turn_index, c.part_index, c.kind, c.content, c.truncated
                   FROM turn t JOIN turn_content c ON c.turn_rowid = t.rowid
                  WHERE t.environment_key = ?1 AND t.agent = ?2 AND t.session_id = ?3
                    AND t.claim_fence = ?4
                  ORDER BY t.source_key, t.turn_index, c.part_index",
            )
            .expect("prepare turn_content query");
        statement
            .query_map(
                rusqlite::params![key.environment_key, key.agent, key.session_id, claim_fence],
                |row| {
                    let truncated: i64 = row.get(5)?;
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        truncated != 0,
                    ))
                },
            )
            .expect("query turn_content rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect turn_content rows")
    })
}

/// A composite sink restored from `snapshot`'s metrics, evidence, and row
/// sink index, fanning its rows into `store`. Mirrors the restore step a
/// desktop worker (phase 3b) will run before every resumed pass.
fn restored_composite(
    store: &Arc<MemoryTurnRowStore>,
    session_id: &str,
    snapshot: &StreamSnapshot,
) -> CompositeSink {
    let metrics = SessionMetricsAccumulator::restore(snapshot.metrics.clone());
    let evidence = SessionEvidenceAccumulator::from_coverage_record_with_resume(
        snapshot.evidence.record.clone(),
        snapshot.evidence.resume.clone(),
    );
    let turn_rows = TurnRowSink::new(
        Arc::clone(store) as Arc<dyn TurnRowStore>,
        session_id.to_owned(),
        None,
    )
    .with_start_index(snapshot.next_turn_index);
    CompositeSink::with_turn_rows(metrics, evidence, turn_rows)
}

/// Runs the full path (one `visit_claimed` pass over `path`'s current
/// content) and returns everything [`assert_resume_parity`] compares.
#[allow(clippy::type_complexity)]
fn run_full_pass(
    agent: &str,
    session_id: &str,
    path: &Path,
    capabilities: SourceCapabilities,
) -> (
    Vec<antiburn_local::analysis::TurnRow>,
    Vec<(String, u64, i64, String, Vec<u8>, bool)>,
    antiburn_local::analysis::TurnFacts,
    antiburn_local::analysis::SessionMetrics,
    antiburn_local::analysis::SessionSummary,
    antiburn_local::analysis::SessionEvidence,
) {
    let store = MemoryTurnRowStore::new(agent, session_id);
    let mut composite = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::Jsonl,
            capabilities,
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session_id.to_owned(),
            None,
        ),
    );
    let input = session_input(agent, session_id, path);
    let claim = claim_for(path);
    let outcome = adapter_for(agent)
        .visit_claimed(
            &input,
            &claim,
            AppendOnlyGuarantee::Absent,
            &|| false,
            &mut composite,
        )
        .expect("full pass must stream");
    assert_eq!(outcome, VisitOutcome::AcceptedFull, "full pass outcome");
    composite.observe_source_outcome(outcome);
    assert!(
        !composite.turn_row_write_failed(),
        "full pass: turn row write must not fail"
    );

    let key = TurnSessionKey {
        environment_key: "native",
        agent,
        session_id,
    };
    let (rows, facts) = store.with_connection(|conn| {
        let rows =
            query_turn_rows(conn, &key, &FenceScope::single(1)).expect("full pass rows query");
        let facts =
            query_turn_facts(conn, &key, &FenceScope::single(1)).expect("full pass facts query");
        (rows, facts)
    });
    let content = turn_content_rows(&store, &key, 1);
    let metrics = composite.metrics().expect("full pass must publish metrics");
    let summary = composite.summary().cloned().expect("full pass must finish");
    let evidence = composite
        .evidence()
        .expect("full pass must publish evidence");
    (rows, content, facts, metrics, summary, evidence)
}

/// Streams `steps` (one record-aligned, growing prefix per step) into
/// `path`, and at every step compares the resumed path (a snapshot carried
/// forward from `S0`) against the full path (one fresh `visit_claimed` over
/// that step's whole content).
///
/// A step whose resumed visit carries no snapshot forward (`visit.resume`
/// is `None` — an adapter's "unsettled" rule, such as Codex fork ownership
/// still `Pending` at EOF) still settles and still must match the full pass
/// for that step. The next step then starts from a fresh bootstrap pass
/// instead of a continuation: a fresh store and a fresh, offset-zero
/// snapshot, exactly the fallback a caller with no stored snapshot takes.
fn assert_resume_parity(
    agent: &str,
    session_id: &str,
    capabilities: SourceCapabilities,
    steps: &[String],
) {
    assert!(!steps.is_empty(), "at least one step is required");
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("session.jsonl");
    let adapter = adapter_for(agent);

    let mut resumed_store = MemoryTurnRowStore::new(agent, session_id);
    let mut snapshot = fresh_snapshot(agent, session_id, capabilities);

    for (index, step) in steps.iter().enumerate() {
        std::fs::write(&path, step).expect("write step content");
        let input = session_input(agent, session_id, &path);
        let claim = claim_for(&path);

        let mut resumed = restored_composite(&resumed_store, session_id, &snapshot);
        let visit = adapter
            .visit_claimed_resumed(&input, &claim, &snapshot, &|| false, &mut resumed)
            .unwrap_or_else(|error| {
                panic!("{agent}/{session_id} step {index}: resumed visit failed: {error:?}")
            });
        assert_eq!(
            visit.outcome,
            VisitOutcome::AcceptedFull,
            "{agent}/{session_id} step {index}: resumed outcome"
        );
        resumed.observe_source_outcome(visit.outcome);
        assert!(
            !resumed.turn_row_write_failed(),
            "{agent}/{session_id} step {index}: resumed row write must not fail"
        );

        let key = TurnSessionKey {
            environment_key: "native",
            agent,
            session_id,
        };
        let (resumed_rows, resumed_facts) = resumed_store.with_connection(|conn| {
            let rows =
                query_turn_rows(conn, &key, &FenceScope::single(1)).expect("resumed rows query");
            let facts =
                query_turn_facts(conn, &key, &FenceScope::single(1)).expect("resumed facts query");
            (rows, facts)
        });
        let resumed_content = turn_content_rows(&resumed_store, &key, 1);
        let resumed_metrics = resumed
            .metrics()
            .expect("resumed pass must publish metrics");
        let resumed_summary = resumed
            .summary()
            .cloned()
            .expect("resumed pass must finish");
        let resumed_evidence = resumed
            .evidence()
            .expect("resumed pass must publish evidence");

        let (full_rows, full_content, full_facts, full_metrics, full_summary, full_evidence) =
            run_full_pass(agent, session_id, &path, capabilities);

        assert_eq!(
            resumed_rows, full_rows,
            "{agent}/{session_id} step {index}: turn rows"
        );
        assert_eq!(
            resumed_content, full_content,
            "{agent}/{session_id} step {index}: turn content"
        );
        assert_eq!(
            resumed_facts, full_facts,
            "{agent}/{session_id} step {index}: TurnFacts"
        );
        assert_eq!(
            resumed_metrics, full_metrics,
            "{agent}/{session_id} step {index}: SessionMetrics"
        );
        assert_eq!(
            resumed_summary, full_summary,
            "{agent}/{session_id} step {index}: SessionSummary"
        );
        assert_eq!(
            resumed_evidence, full_evidence,
            "{agent}/{session_id} step {index}: SessionEvidence"
        );

        match visit.resume {
            Some(resume) => {
                snapshot = resumed.snapshot(resume).unwrap_or_else(|| {
                    panic!(
                        "{agent}/{session_id} step {index}: resumed pass must publish a snapshot to carry state forward"
                    )
                });
            }
            None => {
                resumed_store = MemoryTurnRowStore::new(agent, session_id);
                snapshot = fresh_snapshot(agent, session_id, capabilities);
            }
        }
    }
}

/* --------------------------------------------------------------------
 * Positive cases: a synthetic corpus cut into 5 steps, and every Claude
 * characterization fixture `evidence_replay_parity.rs` sweeps, cut into 3.
 * ----------------------------------------------------------------- */

#[test]
fn a_synthetic_session_resumes_identically_at_every_step() {
    let spec = SessionSpec::tier_s(9001, 0, 47);
    let session = generate_session(&spec);
    let steps = record_aligned_steps(&session.jsonl, 5);
    assert_resume_parity(
        "claude",
        &session.session_id,
        SourceCapabilities::claude(),
        &steps,
    );
}

fn claude_fixture_names() -> [&'static str; 28] {
    [
        "records_all_kinds",
        "timestamps_repeated_and_out_of_order",
        "malformed_between_valid",
        "incomplete_final_record",
        "unrecognized_type",
        "unrecognized_role_with_usage",
        "unrecognized_evidence_shapes",
        "unrecognized_inert_records",
        "unrecognized_inert_sidechain",
        "parent_with_task_spawn",
        "subagent_child",
        "multi_model_session",
        "compaction_with_cache_rehydration",
        "inferred_cache_rehydration",
        "mcp_and_skill_sources",
        "reasoning_and_fast_mode",
        "delegated_turns",
        "delegated_models",
        "delegated_model_missing",
        "housekeeping_records",
        "thread_identity_chain",
        "thread_identity_missing_uuid",
        "sidechain_in_parent",
        "late_skill_metrics",
        "two_compactions_second_without_metadata",
        "rehydration_gap_none",
        "disorder_ladder",
        "subagent_single_timestamp",
    ]
}

#[test]
fn every_claude_characterization_fixture_resumes_identically_in_three_steps() {
    for name in claude_fixture_names() {
        let jsonl = claude_fixture::read_fixture(name);
        let record_count = jsonl.lines().count();
        let step_count = record_count.min(3);
        let steps = record_aligned_steps(&jsonl, step_count);
        assert_resume_parity(
            "claude",
            &format!("resume-{name}"),
            SourceCapabilities::claude(),
            &steps,
        );
    }
}

/// Every `.jsonl` fixture name (without extension) directly inside
/// `tests/fixtures/<dir>/`, sorted for a stable sweep order. Unlike
/// `claude_fixture_names`'s curated list, the Codex and Pi sweeps below
/// cover every fixture their directory holds.
fn characterization_fixture_names(dir: &str) -> Vec<String> {
    let base = format!("{}/tests/fixtures/{dir}", env!("CARGO_MANIFEST_DIR"));
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .unwrap_or_else(|error| panic!("read fixture dir {base}: {error}"))
        .map(|entry| entry.expect("read fixture dir entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .map(|path| {
            path.file_stem()
                .expect("fixture file has a stem")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn characterization_fixture(dir: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{dir}/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("fixture must be readable")
}

/// Every Codex characterization fixture, cut into three record-aligned
/// steps. A fixture whose fork ownership is still `Pending` at some cut
/// point exercises the "resume: None → fresh bootstrap pass" fallback in
/// [`assert_resume_parity`]; see `unresolved_fork` and `resolved_fork`.
#[test]
fn every_codex_characterization_fixture_resumes_identically_in_three_steps() {
    for name in characterization_fixture_names("codex_characterization") {
        let jsonl = characterization_fixture("codex_characterization", &name);
        let record_count = jsonl.lines().count();
        let step_count = record_count.min(3);
        let steps = record_aligned_steps(&jsonl, step_count);
        assert_resume_parity(
            "codex",
            &format!("resume-{name}"),
            SourceCapabilities::codex(),
            &steps,
        );
    }
}

/// Every Pi characterization fixture, cut into three record-aligned steps.
#[test]
fn every_pi_characterization_fixture_resumes_identically_in_three_steps() {
    for name in characterization_fixture_names("pi_characterization") {
        let jsonl = characterization_fixture("pi_characterization", &name);
        let record_count = jsonl.lines().count();
        let step_count = record_count.min(3);
        let steps = record_aligned_steps(&jsonl, step_count);
        assert_resume_parity(
            "pi",
            &format!("resume-{name}"),
            SourceCapabilities::pi(),
            &steps,
        );
    }
}

/* --------------------------------------------------------------------
 * Negative cases.
 * ----------------------------------------------------------------- */

#[test]
fn a_rewritten_tail_is_rejected_instead_of_resuming() {
    let spec = SessionSpec::tier_s(9002, 0, 10);
    let session = generate_session(&spec);
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, &session.jsonl).expect("write source");
    let input = session_input("claude", &session.session_id, &path);

    let first_claim = claim_for(&path);
    let mut first_pass = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", &session.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.session_id.clone(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            MemoryTurnRowStore::new("claude", &session.session_id) as Arc<dyn TurnRowStore>,
            session.session_id.clone(),
            None,
        ),
    );
    let visit = ClaudeAdapter
        .visit_claimed_resumed(
            &input,
            &first_claim,
            &fresh_snapshot("claude", &session.session_id, SourceCapabilities::claude()),
            &|| false,
            &mut first_pass,
        )
        .expect("first resumed visit");
    first_pass.observe_source_outcome(visit.outcome);
    let resume = visit.resume.expect("a settled pass carries a resume");
    let snapshot = first_pass
        .snapshot(resume)
        .expect("first pass must publish a snapshot");

    // Same identity, a same-size rewrite that changes bytes inside the tail
    // window `open_resumed` hashes. A fresh claim (matching the rewritten
    // content) isolates this from the unrelated head-region check.
    let rewritten: Vec<u8> = session
        .jsonl
        .bytes()
        .map(|byte| if byte == b'0' { b'1' } else { byte })
        .collect();
    assert_eq!(
        rewritten.len(),
        session.jsonl.len(),
        "rewrite must not resize"
    );
    std::fs::write(&path, &rewritten).expect("rewrite source");
    let rewritten_claim = claim_for(&path);
    let second_store = MemoryTurnRowStore::new("claude", &session.session_id);
    let mut second_composite = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", &session.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.session_id.clone(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&second_store) as Arc<dyn TurnRowStore>,
            session.session_id.clone(),
            None,
        ),
    );

    let result = ClaudeAdapter
        .visit_claimed_resumed(
            &input,
            &rewritten_claim,
            &snapshot,
            &|| false,
            &mut second_composite,
        )
        .expect("resumed visit of a rewritten source");

    assert_eq!(
        result.outcome,
        VisitOutcome::SourceChanged(SourceChangedReason::ResumeTailMismatch)
    );
    assert!(result.resume.is_none());
}

#[test]
fn a_truncation_below_the_resume_offset_is_rejected_instead_of_resuming() {
    let spec = SessionSpec::tier_s(9003, 0, 10);
    let session = generate_session(&spec);
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, &session.jsonl).expect("write source");
    let input = session_input("claude", &session.session_id, &path);

    let first_claim = claim_for(&path);
    let store = MemoryTurnRowStore::new("claude", &session.session_id);
    let mut first_pass = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", &session.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.session_id.clone(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session.session_id.clone(),
            None,
        ),
    );
    let visit = ClaudeAdapter
        .visit_claimed_resumed(
            &input,
            &first_claim,
            &fresh_snapshot("claude", &session.session_id, SourceCapabilities::claude()),
            &|| false,
            &mut first_pass,
        )
        .expect("first resumed visit");
    first_pass.observe_source_outcome(visit.outcome);
    let resume = visit.resume.expect("a settled pass carries a resume");
    let snapshot = first_pass
        .snapshot(resume)
        .expect("first pass must publish a snapshot");

    // Truncate below the resume offset: a new claim over the shortened
    // file, boundary-only (no head-hash change), isolates the offset check
    // this test exercises from the unrelated head-region check.
    let truncated_len = snapshot.resume.offset / 2;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open source for truncation");
    file.set_len(truncated_len).expect("truncate source");
    drop(file);
    let truncated_claim = SourceClaim {
        boundary: 0,
        head_hash: None,
        ..first_claim
    };
    let store = MemoryTurnRowStore::new("claude", &session.session_id);
    let mut second_pass = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", &session.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.session_id.clone(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session.session_id.clone(),
            None,
        ),
    );

    let result = ClaudeAdapter
        .visit_claimed_resumed(
            &input,
            &truncated_claim,
            &snapshot,
            &|| false,
            &mut second_pass,
        )
        .expect("resumed visit of a truncated source");

    assert_eq!(
        result.outcome,
        VisitOutcome::SourceChanged(SourceChangedReason::ResumeTailMismatch)
    );
    assert!(result.resume.is_none());
}

#[test]
fn a_stale_snapshot_revision_is_rejected_by_is_current_and_by_the_adapter() {
    let spec = SessionSpec::tier_s(9004, 0, 10);
    let session = generate_session(&spec);
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, &session.jsonl).expect("write source");
    let input = session_input("claude", &session.session_id, &path);
    let claim = claim_for(&path);

    let mut stale = fresh_snapshot("claude", &session.session_id, SourceCapabilities::claude());
    stale.revision = RESUME_SNAPSHOT_REVISION - 1;
    assert!(!stale.is_current());

    let store = MemoryTurnRowStore::new("claude", &session.session_id);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", &session.session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.session_id.clone(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session.session_id.clone(),
            None,
        ),
    );

    let result = ClaudeAdapter.visit_claimed_resumed(&input, &claim, &stale, &|| false, &mut sink);
    assert!(result.is_err());
}
