//! Parity check for verified-resume ingest (continuous ingest, phase 3a).
//!
//! A file grows in record-aligned steps `S0 ⊂ S1 ⊂ ... ⊂ Sn`. "Resumed at
//! step k" — a full pass over `S0` with a snapshot, then `visit_claimed_resumed`
//! once per later step through `k`, each time restoring the metrics and
//! evidence accumulators and the row sink's next index from the previous
//! step's snapshot — must equal "a full pass at `Sk`" for turn rows (every
//! column, plus their `turn_content`), `SessionMetrics`, `SessionSummary`,
//! `TurnFacts`, and `SessionEvidence`.
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
    AdapterResume, AdapterSnapshot, AppendOnlyGuarantee, ClaudeSessionReader, CompositeSink,
    EvidenceSnapshot, EvidenceSource, FenceScope, MemoryTurnRowStore, RESUME_SNAPSHOT_REVISION,
    RawSource, RecordSink, ResumePoint, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SessionSummary, SourceCapabilities, SourceChangedReason,
    SourceClaim, SourceFormat, SourceKind, StreamSnapshot, TurnRowSink, TurnRowStore,
    TurnSessionKey, UserMessage, UserMessageChange, UserMessageCompleteness, UserMessageOrigin,
    UserMessagePart, UserMessagePosition, UserMessageScope, VisitOutcome, query_turn_facts,
    query_turn_rows, reader_for,
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
        adapter: reader_for(agent)
            .empty_resume_state()
            .expect("adapter under test must support resume"),
        metrics: SessionMetricsAccumulator::new(agent, session_id),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: Default::default(),
        },
        next_turn_index: 0,
        tool_result_observations_json: Vec::new(),
        native_tool_result_conflicts: Vec::new(),
        tool_result_observations_complete: true,
        user_messages_json: Vec::new(),
        user_message_conflicts: Vec::new(),
        user_messages_complete: true,
    }
}

fn session_input(agent: &str, session_id: &str, path: &Path) -> SessionInput {
    SessionInput {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::File(path.to_path_buf()),
        fork_parent_session_id: None,
        source_format: Default::default(),
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

type TurnContentRow = (
    String,
    u64,
    i64,
    String,
    Vec<u8>,
    bool,
    String,
    Option<String>,
    Option<String>,
);

/// Every `turn_content` row for `key` at `claim_fence`, ordered the same way
/// `query_turn_rows` orders its rows. `query_turn_rows` never joins this
/// table (its `TurnRow::content` is always empty), so this is the separate
/// content read the module doc promises.
fn turn_content_rows(
    store: &MemoryTurnRowStore,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> Vec<TurnContentRow> {
    store.with_connection(|conn| {
        let mut statement = conn
            .prepare(
                "SELECT t.source_key, t.turn_index, c.part_index, c.kind, c.content, c.truncated,
                        c.authority, c.tool_name, c.tool_call_id
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
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
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
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    composite.restore_tool_result_observations(snapshot);
    composite.restore_user_messages(snapshot);
    composite
}

fn user_message(
    source_format: SourceFormat,
    session_id: &str,
    message_id: &str,
    position: UserMessagePosition,
    origin: UserMessageOrigin,
) -> UserMessage {
    UserMessage {
        source_format,
        session_id: session_id.to_owned(),
        message_id: message_id.to_owned(),
        position,
        origin,
        scope: UserMessageScope::MainSession,
        change: UserMessageChange::Present,
        completeness: UserMessageCompleteness::Complete,
        parts: vec![UserMessagePart {
            item_index: 0,
            native_part_id: (source_format == SourceFormat::OpenCodeSqliteV2)
                .then(|| "part-1".to_owned()),
            text: format!("Request for {message_id}."),
        }],
        proof_revision: 1,
    }
}

fn snapshot_with_user_messages(
    agent: &str,
    session_id: &str,
    source_kind: SourceKind,
    capabilities: SourceCapabilities,
    requests: &[UserMessage],
) -> StreamSnapshot {
    let store = MemoryTurnRowStore::new(agent, session_id);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
            kind: source_kind,
            capabilities,
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session_id.to_owned(),
            None,
        ),
    );
    for request in requests {
        sink.user_message(request.clone());
    }
    sink.record(antiburn_local::analysis::NormalizedRecord::MetricsEvent(
        Box::new(antiburn_local::analysis::NormalizedEvent::new(
            antiburn_local::analysis::Role::Assistant,
        )),
    ));
    sink.observe_source_outcome(VisitOutcome::AcceptedFull);
    sink.finish(SessionSummary::default());
    sink.snapshot(AdapterResume {
        point: ResumePoint {
            offset: 0,
            tail_hash: 0,
            tail_len: 0,
        },
        adapter: AdapterSnapshot(Vec::new()),
    })
    .expect("composite sink snapshot")
}

/// Runs the full path (one `visit_claimed` pass over `path`'s current
/// content) and returns everything [`assert_resume_parity`] compares.
fn run_full_pass(
    agent: &str,
    session_id: &str,
    path: &Path,
    capabilities: SourceCapabilities,
) -> (
    Vec<antiburn_local::analysis::TurnRow>,
    Vec<TurnContentRow>,
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
    let outcome = reader_for(agent)
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
    let adapter = reader_for(agent);

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
                snapshot = StreamSnapshot::decode(&snapshot.encode())
                    .expect("snapshot with native tool results must round trip");
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

fn claude_fixture_names() -> [&'static str; 30] {
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
        "native_tool_shapes",
        "selection_isolation",
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

/// `resume_replay.jsonl` replays its compaction boundary, its
/// `isCompactSummary` record, and two assistant turns (records 8-11) under
/// their original uuids. Splitting after record 9 lands the resume point
/// inside that replay run — before the resumed pass has skipped the last
/// two replayed records — so this proves `ClaudeStreamState::seen_uuids`
/// survives the snapshot round trip and keeps skipping them.
#[test]
fn resume_replay_resumes_identically_when_split_inside_the_replay_run() {
    let jsonl = claude_fixture::read_fixture("resume_replay");
    let lines: Vec<&str> = jsonl.lines().collect();
    assert_eq!(
        lines.len(),
        12,
        "resume_replay.jsonl's record count moved; update the split point"
    );
    // Build both steps from the split lines, so a CRLF checkout on Windows
    // cannot make the full step's bytes disagree with the prefix's bytes.
    let step_1 = lines[..9].join("\n") + "\n";
    let step_2 = lines.join("\n") + "\n";
    assert_resume_parity(
        "claude",
        "resume-replay-split-inside-replay",
        SourceCapabilities::claude(),
        &[step_1, step_2],
    );
}

#[test]
fn tool_result_observations_survive_snapshot_resume_and_remain_private() {
    use antiburn_local::analysis::{
        SessionSummary, SourceFormat, ToolResultChange, ToolResultCompleteness,
        ToolResultObservation, ToolResultPosition, ToolResultStatus,
    };

    let session_id = "native-tool-result-resume";
    let store = MemoryTurnRowStore::new("claude", session_id);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session_id.to_owned(),
            None,
        ),
    );
    let result = ToolResultObservation {
        context: None,
        source_format: SourceFormat::ClaudeJsonl,
        session_id: session_id.to_owned(),
        record_id: Some("record-1".to_owned()),
        call_id: Some("call-1".to_owned()),
        result_id: Some("result-1".to_owned()),
        position: ToolResultPosition::Jsonl {
            record_index: 1,
            item_index: 0,
        },
        revision: 1,
        change: ToolResultChange::Update,
        tool_name: Some("Bash".to_owned()),
        tool_category: None,
        arguments: Some(serde_json::json!({"command": "true"})),
        content: Some("ok".to_owned()),
        status: ToolResultStatus::Success,
        finality: true,
        completeness: ToolResultCompleteness::Complete,
    };
    sink.record(antiburn_local::analysis::NormalizedRecord::ToolResult(
        Box::new(result.clone()),
    ));
    sink.record(antiburn_local::analysis::NormalizedRecord::MetricsEvent(
        Box::new(antiburn_local::analysis::NormalizedEvent::new(
            antiburn_local::analysis::Role::Assistant,
        )),
    ));
    sink.observe_source_outcome(VisitOutcome::AcceptedFull);
    sink.finish(SessionSummary::default());
    let snapshot = sink
        .snapshot(antiburn_local::analysis::AdapterResume {
            point: ResumePoint {
                offset: 0,
                tail_hash: 0,
                tail_len: 0,
            },
            adapter: reader_for("claude")
                .empty_resume_state()
                .expect("Claude supports resume"),
        })
        .expect("sink snapshot");
    let snapshot = StreamSnapshot::decode(&snapshot.encode()).expect("decode snapshot");

    let mut resumed = restored_composite(&store, session_id, &snapshot);
    resumed.record(antiburn_local::analysis::NormalizedRecord::ToolResult(
        Box::new(result),
    ));
    assert_eq!(resumed.tool_result_observations().len(), 1);
    assert!(resumed.tool_result_observations_complete());

    let fixture = claude_fixture::read_fixture("native_tool_shapes");
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("native-tool-results.jsonl");
    std::fs::write(&path, fixture).expect("write native result fixture");
    let (_, _, _, _, _, evidence) =
        run_full_pass("claude", session_id, &path, SourceCapabilities::claude());
    assert!(
        !serde_json::to_string(&evidence)
            .expect("serialize public evidence")
            .contains("tool_result_observations")
    );
}

#[test]
fn pi_branch_append_invalidates_results_restored_from_a_snapshot() {
    use antiburn_local::analysis::ToolResultAdmission;

    let prefix = concat!(
        "{\"type\":\"session\",\"version\":3,\"id\":\"s\",\"timestamp\":100}\n",
        "{\"type\":\"message\",\"id\":\"request\",\"parentId\":null,\"timestamp\":200,\"message\":{\"role\":\"user\",\"content\":\"Inspect the failure.\"}}\n",
        "{\"type\":\"message\",\"id\":\"call\",\"parentId\":\"request\",\"timestamp\":300,\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"toolCall\",\"id\":\"tool\",\"name\":\"bash\",\"arguments\":{\"command\":\"false\"}}]}}\n",
        "{\"type\":\"message\",\"id\":\"result\",\"parentId\":\"call\",\"timestamp\":400,\"message\":{\"role\":\"toolResult\",\"toolCallId\":\"tool\",\"toolName\":\"bash\",\"isError\":true,\"content\":[{\"type\":\"text\",\"text\":\"failed\"}]}}\n",
    );
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("branch.jsonl");
    let store = MemoryTurnRowStore::new("pi", "s");
    let mut snapshot = fresh_snapshot("pi", "s", SourceCapabilities::pi());
    for (index, source) in [prefix.to_owned(), format!("{prefix}{{\"type\":\"label\",\"id\":\"branch\",\"parentId\":\"call\",\"timestamp\":500}}\n")].iter().enumerate() {
        std::fs::write(&path, source).unwrap();
        let mut sink = restored_composite(&store, "s", &snapshot);
        let input = SessionInput { source_format: SourceFormat::PiV3Jsonl, ..session_input("pi", "s", &path) };
        let visit = reader_for("pi").visit_claimed_resumed(&input, &claim_for(&path), &snapshot, &|| false, &mut sink).unwrap();
        sink.observe_source_outcome(visit.outcome);
        assert_eq!(sink.tool_result_observations().len(), 1);
        let result = &sink.tool_result_observations()[0];
        assert_eq!(result.admission(), if index == 0 { ToolResultAdmission::Admitted } else { ToolResultAdmission::UnsafeOrder });
        assert_eq!(result.result_id.as_deref(), Some("result"));
        snapshot = sink.snapshot(visit.resume.unwrap()).unwrap();
        snapshot = StreamSnapshot::decode(&snapshot.encode()).unwrap();
    }
}

#[test]
fn user_messages_keep_native_identity_and_position_across_snapshot_resume() {
    let cases = [
        (
            "claude",
            "native-request-claude",
            SourceKind::Jsonl,
            SourceCapabilities::claude(),
            user_message(
                SourceFormat::ClaudeJsonl,
                "native-request-claude",
                "claude-message",
                UserMessagePosition::Jsonl { record_index: 7 },
                UserMessageOrigin::ClaudeUserContent,
            ),
        ),
        (
            "codex",
            "native-request-codex",
            SourceKind::Jsonl,
            SourceCapabilities::codex(),
            user_message(
                SourceFormat::CodexRolloutJsonl,
                "native-request-codex",
                "codex-message",
                UserMessagePosition::Jsonl { record_index: 11 },
                UserMessageOrigin::CodexRetainedUserText,
            ),
        ),
        (
            "opencode",
            "native-request-opencode",
            SourceKind::Sqlite,
            SourceCapabilities::opencode(),
            user_message(
                SourceFormat::OpenCodeSqliteV2,
                "native-request-opencode",
                "opencode-message",
                UserMessagePosition::OpenCodeSqlite {
                    created_at_ms: 1234,
                },
                UserMessageOrigin::OpenCodeUserText,
            ),
        ),
    ];

    for (agent, session_id, kind, capabilities, request) in cases {
        let snapshot = snapshot_with_user_messages(
            agent,
            session_id,
            kind,
            capabilities,
            std::slice::from_ref(&request),
        );
        let snapshot = StreamSnapshot::decode(&snapshot.encode()).expect("decode snapshot");
        let mut restored = CompositeSink::new(
            SessionMetricsAccumulator::new(agent, session_id),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: agent.to_owned(),
                session_id: session_id.to_owned(),
                kind,
                capabilities,
            }),
        );
        restored.restore_user_messages(&snapshot);
        restored.user_message(request.clone());

        assert_eq!(restored.user_messages(), &[request]);
        assert!(restored.user_messages_complete());

        let mut unreadable = snapshot.clone();
        unreadable.user_messages_json[0] = "not-json".to_owned();
        let mut restored_unreadable = CompositeSink::new(
            SessionMetricsAccumulator::new(agent, session_id),
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: agent.to_owned(),
                session_id: session_id.to_owned(),
                kind,
                capabilities,
            }),
        );
        restored_unreadable.restore_user_messages(&unreadable);
        assert!(restored_unreadable.user_messages().is_empty());
        assert!(!restored_unreadable.user_messages_complete());
    }
}

#[test]
fn user_message_conflicts_survive_snapshot_resume() {
    let request = user_message(
        SourceFormat::ClaudeJsonl,
        "native-request-conflict",
        "claude-message",
        UserMessagePosition::Jsonl { record_index: 7 },
        UserMessageOrigin::ClaudeUserContent,
    );
    let mut conflicting = request.clone();
    conflicting.parts[0].text.push_str(" changed");
    let snapshot = snapshot_with_user_messages(
        "claude",
        "native-request-conflict",
        SourceKind::Jsonl,
        SourceCapabilities::claude(),
        &[request, conflicting],
    );
    let snapshot = StreamSnapshot::decode(&snapshot.encode()).expect("decode snapshot");
    let mut restored = CompositeSink::new(
        SessionMetricsAccumulator::new("claude", "native-request-conflict"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "native-request-conflict".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
    );
    restored.restore_user_messages(&snapshot);
    assert!(restored.user_messages().is_empty());
    assert!(!restored.user_messages_complete());
}

#[test]
fn result_context_and_user_messages_match_full_parse_at_every_resume_boundary() {
    for (agent, session_id, format, capabilities, source) in [
        (
            "claude",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            SourceCapabilities::claude(),
            include_str!("fixtures/claude_characterization/retained_native_results.jsonl"),
        ),
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            SourceCapabilities::codex(),
            include_str!("fixtures/codex_characterization/paginated_completed.jsonl"),
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            SourceCapabilities::pi(),
            include_str!("fixtures/pi_characterization/core_linear_v3.jsonl"),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        std::fs::write(&path, source).unwrap();
        let mut input = session_input(agent, session_id, &path);
        input.source_format = format;
        let reader = reader_for(agent);
        let fresh = fresh_snapshot(agent, session_id, capabilities);
        let full_store = MemoryTurnRowStore::new(agent, session_id);
        let mut full = restored_composite(&full_store, session_id, &fresh);
        reader.visit(&input, &mut full).unwrap();
        assert!(!full.tool_result_observations().is_empty(), "{agent}");
        assert!(!full.user_messages().is_empty(), "{agent}");
        let resumed_store = MemoryTurnRowStore::new(agent, session_id);
        let mut snapshot = fresh;
        for prefix in record_aligned_steps(source, source.lines().count()) {
            std::fs::write(&path, prefix).unwrap();
            let mut sink = restored_composite(&resumed_store, session_id, &snapshot);
            let resumed = reader
                .visit_claimed_resumed(&input, &claim_for(&path), &snapshot, &|| false, &mut sink)
                .unwrap();
            sink.observe_source_outcome(resumed.outcome);
            snapshot = sink
                .snapshot(resumed.resume.expect("retained root resumes"))
                .unwrap();
            snapshot = StreamSnapshot::decode(&snapshot.encode()).unwrap();
        }
        let restored = restored_composite(&resumed_store, session_id, &snapshot);
        assert_eq!(
            full.tool_result_observations(),
            restored.tool_result_observations(),
            "{agent}"
        );
        assert_eq!(full.user_messages(), restored.user_messages(), "{agent}");
    }
}

#[test]
fn conflict_tracking_overflow_keeps_retained_requests_across_snapshot_resume() {
    let request = user_message(
        SourceFormat::ClaudeJsonl,
        "native-request-overflow",
        "claude-message",
        UserMessagePosition::Jsonl { record_index: 7 },
        UserMessageOrigin::ClaudeUserContent,
    );
    let mut snapshot = snapshot_with_user_messages(
        "claude",
        "native-request-overflow",
        SourceKind::Jsonl,
        SourceCapabilities::claude(),
        std::slice::from_ref(&request),
    );
    snapshot.user_message_conflicts.push(String::new());
    snapshot.user_messages_complete = false;
    let snapshot = StreamSnapshot::decode(&snapshot.encode()).expect("decode snapshot");
    let mut restored = CompositeSink::new(
        SessionMetricsAccumulator::new("claude", "native-request-overflow"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "native-request-overflow".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
    );

    restored.restore_user_messages(&snapshot);

    assert_eq!(restored.user_messages(), &[request]);
    assert!(!restored.user_messages_complete());
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
    let visit = ClaudeSessionReader
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

    let result = ClaudeSessionReader
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
    let visit = ClaudeSessionReader
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

    let result = ClaudeSessionReader
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

    let result =
        ClaudeSessionReader.visit_claimed_resumed(&input, &claim, &stale, &|| false, &mut sink);
    assert!(result.is_err());
}
