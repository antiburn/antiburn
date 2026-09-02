//! Parity check between the live pipeline's `SessionEvidence` and the same
//! evidence rebuilt from a `SessionCoverageRecord` read back through a
//! store. Modelled on `turn_row_replay_parity.rs`. See phase 2, "evidence
//! from rows", in `docs/plans/continuous-session-ingest.md`.
//!
//! For every vendor characterization fixture `turn_facts_parity.rs` sweeps,
//! this streams the fixture once through a `CompositeSink` (metrics +
//! evidence + `TurnRowSink` into a `MemoryTurnRowStore`), writes the
//! coverage record the fold produced, reads the facts and the coverage
//! record back through the store, rebuilds evidence with
//! `evidence_from_facts`, and asserts the result equals the live
//! accumulator's own `SessionEvidence` field by field. The write and read
//! back put the coverage record's JSON round trip under test, which is the
//! part this file needs to prove.
//!
//! Two scenarios below build their rows and accumulators by hand instead of
//! through a characterization fixture: a parent with a discovered child
//! transcript (`observe_child_coverage`), and a parent with a discovered
//! child that could not be read at all (`observe_child_unreadable`). Neither
//! shape appears in the crate's own characterization fixtures — a
//! discovered child is a separate file the desktop app's own
//! `stream_vendor_with_hooks` folds in, which this crate does not depend on
//! — so this file exercises `SessionEvidenceAccumulator`'s public API
//! directly, the same way `evidence_sink.rs`'s own unit tests do.

use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, MemoryTurnRowStore, NormalizedEvent, NormalizedRecord,
    RawSource, Role, SessionEvidence, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SessionSummary, SourceCapabilities, SourceKind, TurnRowSink,
    TurnRowStore, TurnScope, TurnSessionKey, VisitOutcome, adapter_for, evidence_from_facts,
    query_coverage_record, query_turn_facts,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;

/// Records a mismatch when the replayed `SessionEvidence` and the live
/// accumulator's own `SessionEvidence` differ for one field of one fixture.
fn diff<T: std::fmt::Debug + PartialEq>(
    mismatches: &mut Vec<Mismatch>,
    fixture: &str,
    field: &str,
    replayed: T,
    live: T,
) {
    if replayed != live {
        mismatches.push(Mismatch {
            detail: format!(
                "fixture={fixture} field={field}\n  replayed = {replayed:?}\n  live     = {live:?}"
            ),
        });
    }
}

struct Mismatch {
    detail: String,
}

fn assert_no_mismatches(vendor: &str, mismatches: Vec<Mismatch>) {
    let details: Vec<&str> = mismatches
        .iter()
        .map(|mismatch| mismatch.detail.as_str())
        .collect();
    assert!(
        details.is_empty(),
        "{vendor}: {} mismatch(es) between evidence_from_facts and the live accumulator:\n\n{}",
        details.len(),
        details.join("\n\n")
    );
}

/// Compares every top-level `SessionEvidence` field, for one fixture,
/// pushing each mismatch found into `mismatches`.
fn compare_evidence(
    mismatches: &mut Vec<Mismatch>,
    fixture: &str,
    live: &SessionEvidence,
    replayed: &SessionEvidence,
) {
    diff(
        mismatches,
        fixture,
        "schemaRevision",
        replayed.schema_revision,
        live.schema_revision,
    );
    diff(
        mismatches,
        fixture,
        "identity",
        replayed.identity.clone(),
        live.identity.clone(),
    );
    diff(
        mismatches,
        fixture,
        "context",
        replayed.context.clone(),
        live.context.clone(),
    );
    diff(
        mismatches,
        fixture,
        "capabilities",
        replayed.capabilities,
        live.capabilities,
    );
    diff(
        mismatches,
        fixture,
        "coverage",
        replayed.coverage,
        live.coverage,
    );
    diff(
        mismatches,
        fixture,
        "provenance",
        replayed.provenance.clone(),
        live.provenance.clone(),
    );
    diff(
        mismatches,
        fixture,
        "diagnostics",
        replayed.diagnostics.clone(),
        live.diagnostics.clone(),
    );
    diff(
        mismatches,
        fixture,
        "timeRange",
        replayed.time_range.clone(),
        live.time_range.clone(),
    );
    diff(
        mismatches,
        fixture,
        "eligibility",
        replayed.eligibility.clone(),
        live.eligibility.clone(),
    );
    diff(
        mismatches,
        fixture,
        "tools",
        replayed.tools.clone(),
        live.tools.clone(),
    );
    diff(
        mismatches,
        fixture,
        "contextSources",
        replayed.context_sources.clone(),
        live.context_sources.clone(),
    );
    diff(
        mismatches,
        fixture,
        "models",
        replayed.models.clone(),
        live.models.clone(),
    );
    diff(
        mismatches,
        fixture,
        "subagents",
        replayed.subagents.clone(),
        live.subagents.clone(),
    );
    diff(
        mismatches,
        fixture,
        "cache",
        replayed.cache.clone(),
        live.cache.clone(),
    );
    diff(
        mismatches,
        fixture,
        "compactions",
        replayed.compactions.clone(),
        live.compactions.clone(),
    );
    diff(
        mismatches,
        fixture,
        "quotaIncidents",
        replayed.quota_incidents.clone(),
        live.quota_incidents.clone(),
    );
}

/// Streams `input` through the real adapter and the real evidence and
/// turn-row pipeline, then rebuilds `SessionEvidence` from the facts and
/// coverage record read back through the store. Returns `(live, replayed)`.
fn run_fixture_and_replay(
    agent: &str,
    fixture: &str,
    input: &SessionInput,
    capabilities: SourceCapabilities,
) -> (SessionEvidence, SessionEvidence) {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities,
    });
    let store = MemoryTurnRowStore::new(input.agent.clone(), input.session_id.clone());
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for(agent)
        .visit(input, &mut composite)
        .expect("fixture must stream");
    composite.observe_source_outcome(outcome);
    assert!(
        !composite.turn_row_write_failed(),
        "fixture {fixture}: turn row write must not fail"
    );
    let live = composite
        .evidence()
        .expect("finished source must publish evidence");
    let record = composite
        .coverage_record()
        .expect("finished source must publish a coverage record");

    let key = TurnSessionKey {
        environment_key: "native",
        agent,
        session_id: &input.session_id,
    };
    store
        .write_coverage_record(&record)
        .expect("coverage record must write");
    let (facts, record) = store.with_connection(|conn| {
        let facts = query_turn_facts(conn, &key, 1).expect("query facts must succeed");
        let record = query_coverage_record(conn, &key, 1)
            .expect("query coverage record must succeed")
            .expect("coverage record must have been written");
        (facts, record)
    });
    let replayed = evidence_from_facts(&facts, &record);
    (live, replayed)
}

fn run_fixture(
    agent: &str,
    fixture: &str,
    jsonl: &str,
    capabilities: SourceCapabilities,
) -> (SessionEvidence, SessionEvidence) {
    let input = SessionInput {
        agent: agent.to_owned(),
        session_id: fixture.to_owned(),
        source: RawSource::Jsonl(jsonl.to_owned()),
    };
    run_fixture_and_replay(agent, fixture, &input, capabilities)
}

/* --------------------------------------------------------------------
 * Claude fixtures. Same fixture set `turn_facts_parity.rs` sweeps.
 * ----------------------------------------------------------------- */

fn claude_fixture(name: &str) -> &'static str {
    match name {
        "records_all_kinds" => {
            include_str!("fixtures/claude_characterization/records_all_kinds.jsonl")
        }
        "timestamps_repeated_and_out_of_order" => include_str!(
            "fixtures/claude_characterization/timestamps_repeated_and_out_of_order.jsonl"
        ),
        "malformed_between_valid" => {
            include_str!("fixtures/claude_characterization/malformed_between_valid.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/claude_characterization/incomplete_final_record.jsonl")
        }
        "unrecognized_type" => {
            include_str!("fixtures/claude_characterization/unrecognized_type.jsonl")
        }
        "unrecognized_role_with_usage" => {
            include_str!("fixtures/claude_characterization/unrecognized_role_with_usage.jsonl")
        }
        "unrecognized_evidence_shapes" => {
            include_str!("fixtures/claude_characterization/unrecognized_evidence_shapes.jsonl")
        }
        "unrecognized_inert_records" => {
            include_str!("fixtures/claude_characterization/unrecognized_inert_records.jsonl")
        }
        "unrecognized_inert_sidechain" => {
            include_str!("fixtures/claude_characterization/unrecognized_inert_sidechain.jsonl")
        }
        "parent_with_task_spawn" => {
            include_str!("fixtures/claude_characterization/parent_with_task_spawn.jsonl")
        }
        "subagent_child" => include_str!("fixtures/claude_characterization/subagent_child.jsonl"),
        "multi_model_session" => {
            include_str!("fixtures/claude_characterization/multi_model_session.jsonl")
        }
        "compaction_with_cache_rehydration" => {
            include_str!("fixtures/claude_characterization/compaction_with_cache_rehydration.jsonl")
        }
        "inferred_cache_rehydration" => {
            include_str!("fixtures/claude_characterization/inferred_cache_rehydration.jsonl")
        }
        "mcp_and_skill_sources" => {
            include_str!("fixtures/claude_characterization/mcp_and_skill_sources.jsonl")
        }
        "reasoning_and_fast_mode" => {
            include_str!("fixtures/claude_characterization/reasoning_and_fast_mode.jsonl")
        }
        "delegated_turns" => {
            include_str!("fixtures/claude_characterization/delegated_turns.jsonl")
        }
        "delegated_models" => {
            include_str!("fixtures/claude_characterization/delegated_models.jsonl")
        }
        "delegated_model_missing" => {
            include_str!("fixtures/claude_characterization/delegated_model_missing.jsonl")
        }
        "housekeeping_records" => {
            include_str!("fixtures/claude_characterization/housekeeping_records.jsonl")
        }
        "thread_identity_chain" => {
            include_str!("fixtures/claude_characterization/thread_identity_chain.jsonl")
        }
        "thread_identity_missing_uuid" => {
            include_str!("fixtures/claude_characterization/thread_identity_missing_uuid.jsonl")
        }
        "sidechain_in_parent" => {
            include_str!("fixtures/claude_characterization/sidechain_in_parent.jsonl")
        }
        "late_skill_metrics" => {
            include_str!("fixtures/claude_characterization/late_skill_metrics.jsonl")
        }
        "two_compactions_second_without_metadata" => include_str!(
            "fixtures/claude_characterization/two_compactions_second_without_metadata.jsonl"
        ),
        "rehydration_gap_none" => {
            include_str!("fixtures/claude_characterization/rehydration_gap_none.jsonl")
        }
        "disorder_ladder" => {
            include_str!("fixtures/claude_characterization/disorder_ladder.jsonl")
        }
        "subagent_single_timestamp" => {
            include_str!("fixtures/claude_characterization/subagent_single_timestamp.jsonl")
        }
        _ => panic!("unknown Claude characterization fixture: {name}"),
    }
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
fn claude_evidence_from_facts_matches_the_live_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in claude_fixture_names() {
        let (live, replayed) = run_fixture(
            "claude",
            name,
            claude_fixture(name),
            SourceCapabilities::claude(),
        );
        compare_evidence(&mut mismatches, name, &live, &replayed);
    }
    assert_no_mismatches("claude", mismatches);
}

/* --------------------------------------------------------------------
 * Codex fixtures. Same fixture set `turn_facts_parity.rs` sweeps —
 * `resolved_fork` and `fork_developer_lookbehind` exercise a Codex
 * sub-agent (a `collab_agent_spawn_begin` thread inline in the same file).
 * ----------------------------------------------------------------- */

fn codex_fixture(name: &str) -> &'static str {
    match name {
        "records_all_kinds" => {
            include_str!("fixtures/codex_characterization/records_all_kinds.jsonl")
        }
        "malformed_between_valid" => {
            include_str!("fixtures/codex_characterization/malformed_between_valid.jsonl")
        }
        "unrecognized_type" => {
            include_str!("fixtures/codex_characterization/unrecognized_type.jsonl")
        }
        "absent_model_and_effort" => {
            include_str!("fixtures/codex_characterization/absent_model_and_effort.jsonl")
        }
        "resolved_fork" => include_str!("fixtures/codex_characterization/resolved_fork.jsonl"),
        "fork_developer_lookbehind" => {
            include_str!("fixtures/codex_characterization/fork_developer_lookbehind.jsonl")
        }
        "fork_disputed_window" => {
            include_str!("fixtures/codex_characterization/fork_disputed_window.jsonl")
        }
        "unresolved_fork" => {
            include_str!("fixtures/codex_characterization/unresolved_fork.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/codex_characterization/incomplete_final_record.jsonl")
        }
        _ => panic!("unknown Codex characterization fixture: {name}"),
    }
}

fn codex_fixture_names() -> [&'static str; 9] {
    [
        "records_all_kinds",
        "malformed_between_valid",
        "unrecognized_type",
        "absent_model_and_effort",
        "resolved_fork",
        "fork_developer_lookbehind",
        "fork_disputed_window",
        "unresolved_fork",
        "incomplete_final_record",
    ]
}

#[test]
fn codex_evidence_from_facts_matches_the_live_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in codex_fixture_names() {
        let (live, replayed) = run_fixture(
            "codex",
            name,
            codex_fixture(name),
            SourceCapabilities::codex(),
        );
        compare_evidence(&mut mismatches, name, &live, &replayed);
    }
    assert_no_mismatches("codex", mismatches);
}

/* --------------------------------------------------------------------
 * Pi fixtures. Same fixture set `turn_facts_parity.rs` sweeps.
 * ----------------------------------------------------------------- */

fn pi_fixture(name: &str) -> &'static str {
    match name {
        "minimal_session" => include_str!("fixtures/pi_characterization/minimal_session.jsonl"),
        "role_ordering" => include_str!("fixtures/pi_characterization/role_ordering.jsonl"),
        "content_blocks" => include_str!("fixtures/pi_characterization/content_blocks.jsonl"),
        "usage_all_buckets" => {
            include_str!("fixtures/pi_characterization/usage_all_buckets.jsonl")
        }
        "usage_subset_keys" => {
            include_str!("fixtures/pi_characterization/usage_subset_keys.jsonl")
        }
        "model_change" => include_str!("fixtures/pi_characterization/model_change.jsonl"),
        "thinking_level_change" => {
            include_str!("fixtures/pi_characterization/thinking_level_change.jsonl")
        }
        "compaction_and_inert" => {
            include_str!("fixtures/pi_characterization/compaction_and_inert.jsonl")
        }
        "unknown_row_type" => {
            include_str!("fixtures/pi_characterization/unknown_row_type.jsonl")
        }
        "unknown_content_block" => {
            include_str!("fixtures/pi_characterization/unknown_content_block.jsonl")
        }
        "custom_rows" => include_str!("fixtures/pi_characterization/custom_rows.jsonl"),
        "malformed_middle" => {
            include_str!("fixtures/pi_characterization/malformed_middle.jsonl")
        }
        "incomplete_final_record" => {
            include_str!("fixtures/pi_characterization/incomplete_final_record.jsonl")
        }
        "header_only" => include_str!("fixtures/pi_characterization/header_only.jsonl"),
        "unsupported_version" => {
            include_str!("fixtures/pi_characterization/unsupported_version.jsonl")
        }
        "fork_hazard_parent" => {
            include_str!("fixtures/pi_characterization/fork_hazard_parent.jsonl")
        }
        "fork_hazard_child" => {
            include_str!("fixtures/pi_characterization/fork_hazard_child.jsonl")
        }
        "fork_no_inherited" => {
            include_str!("fixtures/pi_characterization/fork_no_inherited.jsonl")
        }
        "timestamp_disagreement" => {
            include_str!("fixtures/pi_characterization/timestamp_disagreement.jsonl")
        }
        "image_block" => include_str!("fixtures/pi_characterization/image_block.jsonl"),
        "bash_execution_role" => {
            include_str!("fixtures/pi_characterization/bash_execution_role.jsonl")
        }
        "bash_execution_with_usage" => {
            include_str!("fixtures/pi_characterization/bash_execution_with_usage.jsonl")
        }
        "mixed_api" => include_str!("fixtures/pi_characterization/mixed_api.jsonl"),
        "skill_tool_privacy" => {
            include_str!("fixtures/pi_characterization/skill_tool_privacy.jsonl")
        }
        "inert_signal_guard" => {
            include_str!("fixtures/pi_characterization/inert_signal_guard.jsonl")
        }
        "non_turn_timestamp_ordering" => {
            include_str!("fixtures/pi_characterization/non_turn_timestamp_ordering.jsonl")
        }
        "session_start" => include_str!("fixtures/pi_characterization/session_start.jsonl"),
        "headerless_tools" => {
            include_str!("fixtures/pi_characterization/headerless_tools.jsonl")
        }
        "headerless_usage" => {
            include_str!("fixtures/pi_characterization/headerless_usage.jsonl")
        }
        _ => panic!("unknown Pi characterization fixture: {name}"),
    }
}

fn pi_fixture_names() -> [&'static str; 28] {
    [
        "minimal_session",
        "role_ordering",
        "content_blocks",
        "usage_all_buckets",
        "usage_subset_keys",
        "model_change",
        "thinking_level_change",
        "compaction_and_inert",
        "unknown_row_type",
        "unknown_content_block",
        "custom_rows",
        "malformed_middle",
        "header_only",
        "unsupported_version",
        "fork_hazard_parent",
        "fork_hazard_child",
        "fork_no_inherited",
        "timestamp_disagreement",
        "image_block",
        "bash_execution_role",
        "bash_execution_with_usage",
        "mixed_api",
        "skill_tool_privacy",
        "inert_signal_guard",
        "non_turn_timestamp_ordering",
        "session_start",
        "headerless_tools",
        "headerless_usage",
    ]
}

#[test]
fn pi_evidence_from_facts_matches_the_live_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in pi_fixture_names() {
        let (live, replayed) = run_fixture("pi", name, pi_fixture(name), SourceCapabilities::pi());
        compare_evidence(&mut mismatches, name, &live, &replayed);
    }
    assert_no_mismatches("pi", mismatches);
}

/* --------------------------------------------------------------------
 * OpenCode: a provider-database source. Reuses two of
 * `turn_facts_parity.rs`'s own scenarios — the SQLite `session`/`message`/
 * `part` schema is synthetic test fixture data, so mirroring its minimal
 * helpers here is acceptable (that file's own module comment makes the
 * same call for `opencode_characterization.rs`).
 * ----------------------------------------------------------------- */

fn opencode_create_database() -> (TempDir, std::path::PathBuf) {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT
             );",
        )
        .expect("schema");
    drop(connection);
    (directory, path)
}

fn opencode_insert_session(
    connection: &Connection,
    id: &str,
    parent: Option<&str>,
    timestamp: i64,
) {
    connection
        .execute(
            "INSERT INTO session (id, parent_id, title, time_created, time_updated)
             VALUES (?1, ?2, NULL, ?3, ?3)",
            params![id, parent, timestamp],
        )
        .expect("session");
}

fn opencode_insert_message(
    connection: &Connection,
    id: &str,
    session_id: &str,
    timestamp: i64,
    data: &str,
) {
    connection
        .execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
            params![id, session_id, timestamp, data],
        )
        .expect("message");
}

fn opencode_sqlite_input(path: &Path, session_id: &str) -> SessionInput {
    SessionInput {
        agent: "opencode".to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Sqlite(path.to_owned()),
    }
}

/// A root session with a delegated child session and a model transition.
/// Exercises `subagents`, `models.byModel`, and the cache group's
/// model-transition and idle-gap fields, over a real provider-database
/// source.
fn opencode_fixture_subagent_delegation_with_model_transition() -> (TempDir, SessionInput) {
    let (directory, path) = opencode_create_database();
    let connection = Connection::open(&path).expect("database");
    opencode_insert_session(&connection, "root", None, 10);
    opencode_insert_message(
        &connection,
        "r1",
        "root",
        10,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    opencode_insert_session(&connection, "child", Some("root"), 20);
    opencode_insert_message(
        &connection,
        "c1",
        "child",
        20,
        r#"{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":2}}"#,
    );
    opencode_insert_message(
        &connection,
        "r2",
        "root",
        60,
        r#"{"role":"assistant","modelID":"model-c","tokens":{"input":1,"output":1}}"#,
    );
    drop(connection);
    let input = opencode_sqlite_input(&path, "root");
    (directory, input)
}

#[test]
fn opencode_evidence_from_facts_matches_the_live_accumulator_for_a_provider_database_source() {
    let mut mismatches = Vec::new();
    let (_directory, input) = opencode_fixture_subagent_delegation_with_model_transition();
    let (live, replayed) = run_fixture_and_replay(
        "opencode",
        "subagent_delegation_with_model_transition",
        &input,
        SourceCapabilities::opencode(),
    );
    compare_evidence(
        &mut mismatches,
        "subagent_delegation_with_model_transition",
        &live,
        &replayed,
    );
    assert_no_mismatches("opencode", mismatches);
}

/* --------------------------------------------------------------------
 * Discovered children: a separate transcript the desktop app's own
 * `stream_vendor_with_hooks` folds in through `observe_child_coverage` /
 * `observe_child_unreadable`. Built by hand — see this file's module
 * comment.
 * ----------------------------------------------------------------- */

fn one_turn_row(model: &str, ts_ms: i64, uuid: &str) -> NormalizedEvent {
    let mut event = NormalizedEvent::new(Role::Assistant);
    event.ts_ms = Some(ts_ms);
    event.model = Some(model.to_owned());
    event.usage.input_tokens = 10;
    event.usage.output_tokens = 5;
    event.uuid = Some(uuid.to_owned());
    event
}

#[test]
fn a_parent_with_a_discovered_child_matches_evidence_from_facts() {
    let store = MemoryTurnRowStore::new("claude", "parent-1");

    let mut parent_evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: "parent-1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let mut parent_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "parent-1".to_owned(),
        None,
    );
    let parent_record =
        NormalizedRecord::MetricsEvent(Box::new(one_turn_row("claude-opus-4-6", 1_000, "p-1")));
    parent_evidence.observe(&parent_record);
    parent_rows.observe(&parent_record);
    parent_rows.flush();
    parent_evidence.observe_source_outcome(VisitOutcome::AcceptedFull);
    parent_evidence.observe_summary(&SessionSummary::default());

    let mut child_evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: "child-1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let mut child_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "child-1".to_owned(),
        Some(TurnScope::Delegated),
    );
    let child_record =
        NormalizedRecord::MetricsEvent(Box::new(one_turn_row("claude-haiku-4-6", 2_000, "c-1")));
    child_evidence.observe(&child_record);
    child_rows.observe(&child_record);
    child_rows.flush();
    child_evidence.observe_source_outcome(VisitOutcome::AcceptedFull);
    child_evidence.observe_summary(&SessionSummary::default());

    parent_evidence.observe_child_coverage(&child_evidence);

    let facts = store.query_turn_facts().expect("query facts must succeed");
    let live = parent_evidence.evidence(&facts);
    let record = parent_evidence.coverage_record();

    let key = TurnSessionKey {
        environment_key: "native",
        agent: "claude",
        session_id: "parent-1",
    };
    store
        .write_coverage_record(&record)
        .expect("coverage record must write");
    let (facts, record) = store.with_connection(|conn| {
        let facts = query_turn_facts(conn, &key, 1).expect("query facts must succeed");
        let record = query_coverage_record(conn, &key, 1)
            .expect("query coverage record must succeed")
            .expect("coverage record must have been written");
        (facts, record)
    });
    let replayed = evidence_from_facts(&facts, &record);

    let mut mismatches = Vec::new();
    compare_evidence(
        &mut mismatches,
        "parent_with_discovered_child",
        &live,
        &replayed,
    );
    assert_no_mismatches("parent_with_discovered_child", mismatches);
}

#[test]
fn a_parent_with_an_unreadable_child_matches_evidence_from_facts() {
    let store = MemoryTurnRowStore::new("codex", "parent-2");

    let mut parent_evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "codex".to_owned(),
        session_id: "parent-2".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::codex(),
    });
    let mut parent_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "parent-2".to_owned(),
        None,
    );
    let parent_record =
        NormalizedRecord::MetricsEvent(Box::new(one_turn_row("gpt-5-codex", 1_000, "p-1")));
    parent_evidence.observe(&parent_record);
    parent_rows.observe(&parent_record);
    parent_rows.flush();
    parent_evidence.observe_source_outcome(VisitOutcome::AcceptedFull);
    parent_evidence.observe_summary(&SessionSummary::default());
    parent_evidence.observe_child_unreadable();

    let facts = store.query_turn_facts().expect("query facts must succeed");
    let live = parent_evidence.evidence(&facts);
    let record = parent_evidence.coverage_record();

    let key = TurnSessionKey {
        environment_key: "native",
        agent: "codex",
        session_id: "parent-2",
    };
    store
        .write_coverage_record(&record)
        .expect("coverage record must write");
    let (facts, record) = store.with_connection(|conn| {
        let facts = query_turn_facts(conn, &key, 1).expect("query facts must succeed");
        let record = query_coverage_record(conn, &key, 1)
            .expect("query coverage record must succeed")
            .expect("coverage record must have been written");
        (facts, record)
    });
    let replayed = evidence_from_facts(&facts, &record);

    let mut mismatches = Vec::new();
    compare_evidence(
        &mut mismatches,
        "parent_with_unreadable_child",
        &live,
        &replayed,
    );
    assert_no_mismatches("parent_with_unreadable_child", mismatches);
}
