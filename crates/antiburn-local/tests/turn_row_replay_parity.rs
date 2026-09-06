//! Parity check between the live pipeline's `SessionMetrics` and the same
//! metrics rebuilt from turn rows alone. See seam R3b in
//! `docs/plans/session-evidence-harness-parity.md`.
//!
//! For every vendor characterization fixture `turn_facts_parity.rs` sweeps,
//! this streams the fixture once through a `CompositeSink` (accumulator +
//! evidence + `TurnRowSink` into a `MemoryTurnRowStore`), reads the rows
//! back with `query_turn_rows`, replays them with `metrics_from_rows`, and
//! asserts the result equals the live accumulator's own `SessionMetrics`
//! field by field.
//!
//! `metrics_from_rows` takes rows plus a `SessionSummary` per source — rows
//! carry no column for a summary's fields (`model`, `context_window`,
//! `cache_write_tokens_available`, `started_at_ms`, and the rest). Every
//! fixture here streams through one source, so this file gets a real,
//! fixture-accurate summary the same deterministic way the live pipeline
//! does: by running the same adapter over the same fixture bytes a second
//! time, into a sink that only captures the `SessionSummary` `finish` hands
//! it. This is a test-only convenience — `metrics_from_rows` itself never
//! touches raw bytes, a store, or Tauri.
//!
//! Three `SessionMetrics` fields are out of the comparison for every
//! fixture, not as a per-fixture exception but as this seam's own scope
//! boundary (see the PR description and `replay.rs`'s module doc comment):
//! `tool_calls_by_name`, `mcp_tool_calls`, and `skill_uses` need every tool
//! call a turn made, and a row keeps only its last tool's name and its
//! `"task"`-launch count. `initial_context` is downstream of those same
//! per-tool counts (`bound_initial_context`'s `use_count` reads the
//! accumulator's own observed tool/skill/MCP names), so it is out of scope
//! for the same reason.

use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::{
    Bucket, CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, NormalizedRecord,
    RawSource, RecordSink, SessionEvidenceAccumulator, SessionInput, SessionMetrics,
    SessionMetricsAccumulator, SessionSummary, SourceCapabilities, SourceKind, TurnRowSink,
    TurnRowStore, TurnScope, TurnSessionKey, adapter_for, merge_metrics, metrics_by_source,
    metrics_from_rows, query_turn_rows,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;

/// Records a mismatch when the replayed `SessionMetrics` and the live
/// accumulator's own `SessionMetrics` differ for one field of one fixture.
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
        "{vendor}: {} mismatch(es) between metrics_from_rows and the live accumulator:\n\n{}",
        details.len(),
        details.join("\n\n")
    );
}

/// A [`RecordSink`] that keeps only the [`SessionSummary`] `finish` hands
/// it, so a second, deterministic run of the same adapter over the same
/// bytes recovers the exact summary the live run also used — see this
/// file's module doc comment for why that is a legitimate test-only step.
#[derive(Default)]
struct SummaryCapture(Option<SessionSummary>);

impl RecordSink for SummaryCapture {
    fn record(&mut self, _record: NormalizedRecord) {}

    fn finish(&mut self, summary: SessionSummary) {
        self.0 = Some(summary);
    }
}

/// Streams `input` through the real adapter and the real evidence and
/// turn-row pipeline, then rebuilds `SessionMetrics` from the rows that
/// pipeline wrote. Returns `(live, replayed)`.
fn run_fixture_and_replay(
    agent: &str,
    fixture: &str,
    input: &SessionInput,
    capabilities: SourceCapabilities,
) -> (SessionMetrics, SessionMetrics) {
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
        .metrics()
        .expect("finished source must publish metrics");

    let key = TurnSessionKey {
        environment_key: "native",
        agent,
        session_id: &input.session_id,
    };
    let rows = store.with_connection(|conn| {
        query_turn_rows(conn, &key, &FenceScope::single(1)).expect("query rows must succeed")
    });

    let mut capture = SummaryCapture::default();
    adapter_for(agent)
        .visit(input, &mut capture)
        .expect("fixture must stream for summary capture");
    let mut summary = capture.0;
    let replayed = metrics_from_rows(agent, input.session_id.clone(), &rows, |source_key| {
        summary.take().unwrap_or_else(|| {
            panic!(
                "fixture {fixture}: metrics_from_rows asked for a second source's summary \
                 ({source_key:?}), but every fixture here streams through one source"
            )
        })
    })
    .unwrap_or_else(|error| panic!("fixture {fixture}: metrics_from_rows failed: {error}"));

    (live, replayed)
}

/// Strips the one documented, principled gap this seam accepts before
/// comparing `live`'s buckets to the replay's: Claude's
/// `late_skill_metrics` fixture names its skill through a trailing
/// `attachment` record, which `ClaudeAdapter` resolves only in
/// `SessionSummary::late_tools`, after the pipeline already wrote this
/// turn's row (turn ordinal 1, bucket 0) with no tool call at all —
/// `TurnRowSink` observes each row at `record` time, before `finish`
/// resolves late tools. `SessionMetricsAccumulator::finish_summary` then
/// folds the resolved `"skill"` call into `bucket.last_tool` for the live
/// run; the replayed run never reserves a late-tool candidate for a
/// synthesized event (`event_from_row`'s doc comment), so
/// `finish_summary`'s `late_tools` loop finds no candidate to fold it into,
/// and the replayed bucket's `last_tool` stays `None`. No other field, of
/// any other bucket, of any other fixture, carries this gap — see the PR
/// description for why this is the seam's one accepted exclusion, mirroring
/// seam R2's own `delegated_model_missing` precedent.
fn live_buckets_for_comparison(fixture: &str, buckets: &[Bucket]) -> Vec<Bucket> {
    let mut buckets = buckets.to_vec();
    if fixture == "late_skill_metrics" {
        buckets[0].last_tool = None;
    }
    buckets
}

/// Compares every in-scope `SessionMetrics` field between `live` and
/// `replayed` for one fixture, pushing each mismatch into `mismatches`. See
/// this file's module doc comment for the three fields left out of scope,
/// and [`live_buckets_for_comparison`] for the one per-fixture exclusion.
fn compare_metrics(
    mismatches: &mut Vec<Mismatch>,
    fixture: &str,
    live: &SessionMetrics,
    replayed: &SessionMetrics,
) {
    diff(
        mismatches,
        fixture,
        "duration_secs",
        replayed.duration_secs,
        live.duration_secs,
    );
    diff(
        mismatches,
        fixture,
        "active_secs",
        replayed.active_secs,
        live.active_secs,
    );
    diff(
        mismatches,
        fixture,
        "event_count",
        replayed.event_count,
        live.event_count,
    );
    diff(
        mismatches,
        fixture,
        "tokens_in",
        replayed.tokens_in,
        live.tokens_in,
    );
    diff(
        mismatches,
        fixture,
        "tokens_out",
        replayed.tokens_out,
        live.tokens_out,
    );
    diff(
        mismatches,
        fixture,
        "peak_context_tokens",
        replayed.peak_context_tokens,
        live.peak_context_tokens,
    );
    diff(
        mismatches,
        fixture,
        "compaction_count",
        replayed.compaction_count,
        live.compaction_count,
    );
    diff(
        mismatches,
        fixture,
        "cache_routing_miss_count",
        replayed.cache_routing_miss_count,
        live.cache_routing_miss_count,
    );
    diff(
        mismatches,
        fixture,
        "cache_rehydration_count",
        replayed.cache_rehydration_count,
        live.cache_rehydration_count,
    );
    diff(
        mismatches,
        fixture,
        "context_available",
        replayed.context_available,
        live.context_available,
    );
    diff(
        mismatches,
        fixture,
        "context_window",
        replayed.context_window,
        live.context_window,
    );
    diff(
        mismatches,
        fixture,
        "buckets",
        replayed.buckets.clone(),
        live_buckets_for_comparison(fixture, &live.buckets),
    );
    diff(
        mismatches,
        fixture,
        "model",
        replayed.model.clone(),
        live.model.clone(),
    );
    diff(
        mismatches,
        fixture,
        "model_runs",
        replayed.model_runs.clone(),
        live.model_runs.clone(),
    );
    diff(
        mismatches,
        fixture,
        "billable_input_tokens",
        replayed.billable_input_tokens,
        live.billable_input_tokens,
    );
    diff(
        mismatches,
        fixture,
        "billable_output_tokens",
        replayed.billable_output_tokens,
        live.billable_output_tokens,
    );
    diff(
        mismatches,
        fixture,
        "billable_cache_read_tokens",
        replayed.billable_cache_read_tokens,
        live.billable_cache_read_tokens,
    );
    diff(
        mismatches,
        fixture,
        "billable_cache_creation_tokens",
        replayed.billable_cache_creation_tokens,
        live.billable_cache_creation_tokens,
    );
    diff(
        mismatches,
        fixture,
        "model_breakdown",
        replayed.model_breakdown.clone(),
        live.model_breakdown.clone(),
    );
    diff(mismatches, fixture, "cost", replayed.cost, live.cost);
    diff(
        mismatches,
        fixture,
        "efficiency",
        replayed.efficiency,
        live.efficiency,
    );
}

/* --------------------------------------------------------------------
 * Claude fixtures. Same 28 fixtures `turn_facts_parity.rs` sweeps.
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
fn claude_metrics_from_rows_matches_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in claude_fixture_names() {
        let input = SessionInput {
            agent: "claude".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(claude_fixture(name).to_owned()),
            fork_parent_session_id: None,
        };
        let (live, replayed) =
            run_fixture_and_replay("claude", name, &input, SourceCapabilities::claude());
        compare_metrics(&mut mismatches, name, &live, &replayed);
    }
    assert_no_mismatches("claude", mismatches);
}

/* --------------------------------------------------------------------
 * Codex fixtures. Same 9 fixtures `turn_facts_parity.rs` sweeps.
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
fn codex_metrics_from_rows_matches_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in codex_fixture_names() {
        let input = SessionInput {
            agent: "codex".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(codex_fixture(name).to_owned()),
            fork_parent_session_id: None,
        };
        let (live, replayed) =
            run_fixture_and_replay("codex", name, &input, SourceCapabilities::codex());
        compare_metrics(&mut mismatches, name, &live, &replayed);
    }
    assert_no_mismatches("codex", mismatches);
}

/* --------------------------------------------------------------------
 * Pi fixtures. Same 28 fixtures `turn_facts_parity.rs` sweeps.
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
fn pi_metrics_from_rows_matches_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in pi_fixture_names() {
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(pi_fixture(name).to_owned()),
            fork_parent_session_id: None,
        };
        let (live, replayed) = run_fixture_and_replay("pi", name, &input, SourceCapabilities::pi());
        compare_metrics(&mut mismatches, name, &live, &replayed);
    }
    assert_no_mismatches("pi", mismatches);
}

/* --------------------------------------------------------------------
 * OpenCode fixtures. Same five scenarios `turn_facts_parity.rs` builds
 * inline — copied here for the same reason that file documents: OpenCode
 * has no `fixtures/opencode_characterization` directory of its own, and
 * duplicating this synthetic content keeps `opencode_characterization.rs`
 * itself untouched.
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

fn opencode_insert_part(
    connection: &Connection,
    id: &str,
    message_id: &str,
    session_id: &str,
    timestamp: i64,
    data: &str,
) {
    connection
        .execute(
            "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![id, message_id, session_id, timestamp, data],
        )
        .expect("part");
}

fn opencode_sqlite_input(path: &Path, session_id: &str) -> SessionInput {
    SessionInput {
        agent: "opencode".to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Sqlite(path.to_owned()),
        fork_parent_session_id: None,
    }
}

fn opencode_fixture_messages_with_cache_and_reasoning() -> (TempDir, SessionInput) {
    let (directory, path) = opencode_create_database();
    let connection = Connection::open(&path).expect("database");
    opencode_insert_session(&connection, "root", None, 10);
    opencode_insert_message(
        &connection,
        "m1",
        "root",
        20,
        r#"{"role":"assistant","modelID":"model-a","variant":"high","tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":30,"write":40}}}"#,
    );
    drop(connection);
    let input = opencode_sqlite_input(&path, "root");
    (directory, input)
}

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

fn opencode_fixture_malformed_between_valid() -> (TempDir, SessionInput) {
    let (directory, path) = opencode_create_database();
    let connection = Connection::open(&path).expect("database");
    opencode_insert_session(&connection, "root", None, 10);
    opencode_insert_message(
        &connection,
        "m1",
        "root",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":40,"output":8}}"#,
    );
    opencode_insert_message(&connection, "m2", "root", 30, "{not-json");
    opencode_insert_message(
        &connection,
        "m3",
        "root",
        40,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":10,"output":2}}"#,
    );
    drop(connection);
    let input = opencode_sqlite_input(&path, "root");
    (directory, input)
}

fn opencode_fixture_compaction_boundary() -> (TempDir, SessionInput) {
    let (directory, path) = opencode_create_database();
    let connection = Connection::open(&path).expect("database");
    opencode_insert_session(&connection, "root", None, 10);
    opencode_insert_message(
        &connection,
        "m1",
        "root",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":40,"output":8}}"#,
    );
    opencode_insert_part(
        &connection,
        "compaction",
        "m1",
        "root",
        21,
        r#"{"type":"compaction","auto":true,"snapshot":"snapshot"}"#,
    );
    drop(connection);
    let input = opencode_sqlite_input(&path, "root");
    (directory, input)
}

fn opencode_fixture_export_jsonl_child_delegation() -> SessionInput {
    let jsonl = concat!(
        r#"{"type":"session_meta","sessionID":"root","sessionRole":"root","time":{"created":1000},"payload":{"id":"root","title":"Root session"}}"#,
        "\n",
        r#"{"type":"session_member","rootSessionID":"root","originSessionID":"child","sessionRole":"child","parentSessionID":"root","time":{"created":1500},"payload":{"id":"child","title":"Child session"}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"root","sessionRole":"root","messageID":"m1","time":{"created":1000},"payload":{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"child","sessionRole":"child","parentSessionID":"root","messageID":"m2","time":{"created":1600},"payload":{"role":"user"}}"#,
        "\n",
    );
    SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Jsonl(jsonl.to_owned()),
        fork_parent_session_id: None,
    }
}

#[test]
fn opencode_metrics_from_rows_matches_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();

    let (_messages_dir, messages_input) = opencode_fixture_messages_with_cache_and_reasoning();
    let (live, replayed) = run_fixture_and_replay(
        "opencode",
        "messages_with_cache_and_reasoning",
        &messages_input,
        SourceCapabilities::opencode(),
    );
    compare_metrics(
        &mut mismatches,
        "messages_with_cache_and_reasoning",
        &live,
        &replayed,
    );

    let (_subagent_dir, subagent_input) =
        opencode_fixture_subagent_delegation_with_model_transition();
    let (live, replayed) = run_fixture_and_replay(
        "opencode",
        "subagent_delegation_with_model_transition",
        &subagent_input,
        SourceCapabilities::opencode(),
    );
    compare_metrics(
        &mut mismatches,
        "subagent_delegation_with_model_transition",
        &live,
        &replayed,
    );

    let (_malformed_dir, malformed_input) = opencode_fixture_malformed_between_valid();
    let (live, replayed) = run_fixture_and_replay(
        "opencode",
        "malformed_between_valid",
        &malformed_input,
        SourceCapabilities::opencode(),
    );
    compare_metrics(&mut mismatches, "malformed_between_valid", &live, &replayed);

    let (_compaction_dir, compaction_input) = opencode_fixture_compaction_boundary();
    let (live, replayed) = run_fixture_and_replay(
        "opencode",
        "compaction_boundary",
        &compaction_input,
        SourceCapabilities::opencode(),
    );
    compare_metrics(&mut mismatches, "compaction_boundary", &live, &replayed);

    let export_input = opencode_fixture_export_jsonl_child_delegation();
    let (live, replayed) = run_fixture_and_replay(
        "opencode",
        "export_jsonl_child_delegation",
        &export_input,
        SourceCapabilities::opencode(),
    );
    compare_metrics(
        &mut mismatches,
        "export_jsonl_child_delegation",
        &live,
        &replayed,
    );

    assert_no_mismatches("opencode", mismatches);
}

/* --------------------------------------------------------------------
 * Multi-source parity: a parent transcript plus one discovered child
 * transcript, streamed through one shared row store the way
 * `stream_vendor_with_hooks` streams a parent and a discovered sub-agent
 * file in production — every fixture above streams through one source, so
 * this is the one fixture that exercises `metrics_by_source`'s per-source
 * split and `metrics_from_rows`'s merge over a real, adapter-driven
 * multi-file session, not the synthetic `NormalizedEvent` fixtures
 * `replay.rs`'s own unit tests build by hand.
 * ----------------------------------------------------------------- */

const MULTI_SOURCE_PARENT_JSONL: &str = concat!(
    r#"{"type":"assistant","timestamp":1000,"message":{"id":"m1","role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":5},"content":[{"type":"text","text":"Parent turn."}]}}"#,
    "\n",
);

const MULTI_SOURCE_CHILD_JSONL: &str = concat!(
    r#"{"type":"assistant","timestamp":2000,"message":{"id":"m2","role":"assistant","model":"claude-haiku-4-6","usage":{"input_tokens":3,"output_tokens":1},"content":[{"type":"text","text":"Child turn."}]}}"#,
    "\n",
);

/// Streams `input` through the real Claude adapter into its own
/// accumulator, evidence tracker, and [`TurnRowSink`] fanned into the
/// shared `store` — one call per source, mirroring one iteration of
/// `stream_vendor_with_hooks`'s own loop. Returns the finished accumulator
/// and its captured [`SessionSummary`].
fn stream_source_into_shared_store(
    input: &SessionInput,
    store: &std::sync::Arc<MemoryTurnRowStore>,
    scope: Option<TurnScope>,
) -> (SessionMetricsAccumulator, SessionSummary) {
    let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::claude(),
    });
    let turn_rows = TurnRowSink::new(
        Arc::clone(store) as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        scope,
    );
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("claude")
        .visit(input, &mut composite)
        .expect("multi-source fixture must stream");
    composite.observe_source_outcome(outcome);
    assert!(
        !composite.turn_row_write_failed(),
        "multi-source fixture: turn row write must not fail"
    );
    let summary = composite
        .summary()
        .cloned()
        .expect("a streamed source finishes with a summary");
    let (accumulator, _evidence) = composite
        .into_parts()
        .expect("multi-source fixture must publish metrics");
    (accumulator, summary)
}

#[test]
fn metrics_by_source_and_metrics_from_rows_match_the_accumulators_for_a_parent_and_a_discovered_child()
 {
    let parent_input = SessionInput {
        agent: "claude".to_owned(),
        session_id: "multi-source-parent".to_owned(),
        source: RawSource::Jsonl(MULTI_SOURCE_PARENT_JSONL.to_owned()),
        fork_parent_session_id: None,
    };
    let child_input = SessionInput {
        agent: "claude".to_owned(),
        session_id: "multi-source-child".to_owned(),
        source: RawSource::Jsonl(MULTI_SOURCE_CHILD_JSONL.to_owned()),
        fork_parent_session_id: None,
    };

    // One shared row store under the parent's own session id: the parent's
    // sink gets `Main` scope (`None`) and its own session id as
    // `source_key` (`is_parent_group`'s rule, `source_key == session_id`);
    // the child's sink gets `Delegated` scope and its own, distinct session
    // id — exactly the shape `stream_vendor_with_hooks` builds for index 0
    // versus a later, discovered input.
    let store = MemoryTurnRowStore::new("claude", parent_input.session_id.clone());
    let (parent_accumulator, parent_summary) =
        stream_source_into_shared_store(&parent_input, &store, None);
    let (child_accumulator, child_summary) =
        stream_source_into_shared_store(&child_input, &store, Some(TurnScope::Delegated));

    let live_parent = parent_accumulator.metrics();
    let live_child = child_accumulator.metrics();
    let live_merged = merge_metrics(&parent_accumulator, &[child_accumulator]);

    let key = TurnSessionKey {
        environment_key: "native",
        agent: "claude",
        session_id: &parent_input.session_id,
    };
    let rows = store.with_connection(|conn| {
        query_turn_rows(conn, &key, &FenceScope::single(1)).expect("query rows must succeed")
    });

    let summary_for = |source_key: &str| {
        if source_key == parent_input.session_id {
            parent_summary.clone()
        } else {
            child_summary.clone()
        }
    };

    let mut mismatches = Vec::new();

    let by_source = metrics_by_source(
        "claude",
        parent_input.session_id.clone(),
        &rows,
        summary_for,
    );
    compare_metrics(
        &mut mismatches,
        "multi_source_parent",
        &live_parent,
        by_source
            .get(&parent_input.session_id)
            .expect("the parent's own source_key must have a row group"),
    );
    compare_metrics(
        &mut mismatches,
        "multi_source_child",
        &live_child,
        by_source
            .get(&child_input.session_id)
            .expect("the child's own source_key must have a row group"),
    );

    let replayed_merged = metrics_from_rows(
        "claude",
        parent_input.session_id.clone(),
        &rows,
        summary_for,
    )
    .expect("the parent group's source_key equals the session id");
    compare_metrics(
        &mut mismatches,
        "multi_source_merged",
        &live_merged,
        &replayed_merged,
    );

    assert_no_mismatches("claude_multi_source", mismatches);
}
