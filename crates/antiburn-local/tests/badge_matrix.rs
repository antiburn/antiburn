//! Badge matrix acceptance test.
//!
//! One table maps `(harness, fixture, BadgeId)` to the badge status the real
//! pipeline must report: `SessionInput` through the vendor adapter, the
//! composite evidence sink, and `session_badges(&evidence,
//! &ReportCatalogs::default())`. This is the first instalment of the Phase 5
//! acceptance artifact in `docs/plans/session-evidence-harness-parity.md`.
//!
//! Every fixture is synthetic. Existing characterization fixtures cover most
//! cells; the rest are new files under each suite's own fixture directory,
//! registered in that suite's `fixture_names` so their goldens stay frozen.
//! OpenCode has no static fixture files at all (its characterization suite
//! builds every session from an in-memory SQLite database), so its rows
//! build a database inline instead of loading a file.
//!
//! A row that cannot be built yet — because no in-repository test can
//! construct the state, not because a capability is missing — carries a
//! comment instead of an entry, so the table shows the gap.

use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceCoverage, EvidenceSource, MemoryTurnRowStore, RawSource,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SourceCapabilities, SourceKind, TurnRowSink, TurnRowStore, adapter_for,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, DetectorId, NotAssessedReason, ReportCatalogs, clean_facts_complete,
    eligible, session_badges,
};
use rusqlite::{Connection, params};
use tempfile::TempDir;

fn claude_fixture(name: &str) -> &'static str {
    match name {
        "multi_model_session" => {
            include_str!("fixtures/claude_characterization/multi_model_session.jsonl")
        }
        "reasoning_and_fast_mode" => {
            include_str!("fixtures/claude_characterization/reasoning_and_fast_mode.jsonl")
        }
        "delegated_models" => {
            include_str!("fixtures/claude_characterization/delegated_models.jsonl")
        }
        "delegated_turns" => include_str!("fixtures/claude_characterization/delegated_turns.jsonl"),
        "compaction_with_cache_rehydration" => {
            include_str!("fixtures/claude_characterization/compaction_with_cache_rehydration.jsonl")
        }
        "thread_identity_chain" => {
            include_str!("fixtures/claude_characterization/thread_identity_chain.jsonl")
        }
        "malformed_between_valid" => {
            include_str!("fixtures/claude_characterization/malformed_between_valid.jsonl")
        }
        "records_all_kinds" => {
            include_str!("fixtures/claude_characterization/records_all_kinds.jsonl")
        }
        "session_overdepth_finding" => {
            include_str!("fixtures/claude_characterization/session_overdepth_finding.jsonl")
        }
        "model_overthinking_finding" => {
            include_str!("fixtures/claude_characterization/model_overthinking_finding.jsonl")
        }
        "fast_mode_overuse_clean" => {
            include_str!("fixtures/claude_characterization/fast_mode_overuse_clean.jsonl")
        }
        "obsolete_model_finding" => {
            include_str!("fixtures/claude_characterization/obsolete_model_finding.jsonl")
        }
        "obsolete_model_before_effective_date" => include_str!(
            "fixtures/claude_characterization/obsolete_model_before_effective_date.jsonl"
        ),
        "model_overthinking_unrecognized_tier" => include_str!(
            "fixtures/claude_characterization/model_overthinking_unrecognized_tier.jsonl"
        ),
        "model_overthinking_partial_coverage_clean" => include_str!(
            "fixtures/claude_characterization/model_overthinking_partial_coverage_clean.jsonl"
        ),
        "fast_mode_overuse_partial_coverage_clean" => include_str!(
            "fixtures/claude_characterization/fast_mode_overuse_partial_coverage_clean.jsonl"
        ),
        other => panic!("unknown claude fixture: {other}"),
    }
}

fn codex_fixture(name: &str) -> &'static str {
    match name {
        "records_all_kinds" => {
            include_str!("fixtures/codex_characterization/records_all_kinds.jsonl")
        }
        "malformed_between_valid" => {
            include_str!("fixtures/codex_characterization/malformed_between_valid.jsonl")
        }
        "service_tier_priority" => {
            include_str!("fixtures/codex_characterization/service_tier_priority.jsonl")
        }
        "absent_model_and_effort" => {
            include_str!("fixtures/codex_characterization/absent_model_and_effort.jsonl")
        }
        "session_overdepth_finding" => {
            include_str!("fixtures/codex_characterization/session_overdepth_finding.jsonl")
        }
        "model_overthinking_finding" => {
            include_str!("fixtures/codex_characterization/model_overthinking_finding.jsonl")
        }
        "obsolete_model_finding" => {
            include_str!("fixtures/codex_characterization/obsolete_model_finding.jsonl")
        }
        "context_reread" => {
            include_str!("fixtures/codex_characterization/context_reread.jsonl")
        }
        other => panic!("unknown codex fixture: {other}"),
    }
}

fn pi_fixture(name: &str) -> &'static str {
    match name {
        "minimal_session" => include_str!("fixtures/pi_characterization/minimal_session.jsonl"),
        "thinking_level_change" => {
            include_str!("fixtures/pi_characterization/thinking_level_change.jsonl")
        }
        "malformed_middle" => include_str!("fixtures/pi_characterization/malformed_middle.jsonl"),
        "session_overdepth_finding" => {
            include_str!("fixtures/pi_characterization/session_overdepth_finding.jsonl")
        }
        "model_overthinking_finding" => {
            include_str!("fixtures/pi_characterization/model_overthinking_finding.jsonl")
        }
        "excess_cache_rehydration_finding" => {
            include_str!("fixtures/pi_characterization/excess_cache_rehydration_finding.jsonl")
        }
        other => panic!("unknown pi fixture: {other}"),
    }
}

/// Streams one JSONL fixture through the real vendor adapter and the real
/// composite evidence sink, the same path production uses.
fn evidence_for(
    agent: &str,
    capabilities: SourceCapabilities,
    name: &str,
    text: &str,
) -> SessionEvidence {
    let input = SessionInput {
        agent: agent.to_owned(),
        session_id: name.to_owned(),
        source: RawSource::Jsonl(text.to_owned()),
    };
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
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for(agent)
        .visit(&input, &mut sink)
        .expect("fixture must stream");
    sink.observe_source_outcome(outcome);
    sink.evidence().expect("evidence must publish")
}

// ---------------------------------------------------------------------
// OpenCode: no static fixture files. Each named session builds its own
// in-memory SQLite database, mirroring `opencode_characterization.rs`'s own
// `create_database`/`insert_session`/`insert_message` helpers.
// ---------------------------------------------------------------------

fn create_database() -> (TempDir, std::path::PathBuf) {
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

fn insert_session(connection: &Connection, id: &str, parent: Option<&str>, timestamp: i64) {
    connection
        .execute(
            "INSERT INTO session (id, parent_id, title, time_created, time_updated)
             VALUES (?1, ?2, NULL, ?3, ?3)",
            params![id, parent, timestamp],
        )
        .expect("session");
}

fn insert_message(connection: &Connection, id: &str, session_id: &str, timestamp: i64, data: &str) {
    connection
        .execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
            params![id, session_id, timestamp, data],
        )
        .expect("message");
}

fn opencode_evidence(name: &str) -> SessionEvidence {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    match name {
        "session_overdepth_finding" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":450000,"output":50}}"#,
            );
        }
        "session_overdepth_clean" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":40,"output":8}}"#,
            );
        }
        "session_overdepth_malformed" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":40,"output":8}}"#,
            );
            insert_message(&connection, "m2", "root", 30, "{not-json");
        }
        "model_overthinking_finding" => {
            insert_session(&connection, "root", None, 10);
            // A recognized model ID puts the Claude family present, so
            // the reviewed policy can classify `variant` at all.
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"claude-opus-4-6","variant":"max","tokens":{"input":10,"output":5}}"#,
            );
        }
        "model_overthinking_clean" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"claude-sonnet-4-6","variant":"high","tokens":{"input":10,"output":5}}"#,
            );
        }
        "model_overthinking_signal_missing" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":10,"output":5}}"#,
            );
        }
        "overpowered_subagents_finding" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "root-assistant",
                "root",
                20,
                r#"{"role":"assistant","modelID":"claude-opus-4-6","tokens":{"input":10,"output":5}}"#,
            );
            insert_session(&connection, "child", Some("root"), 30);
            insert_message(
                &connection,
                "child-assistant",
                "child",
                40,
                r#"{"role":"assistant","modelID":"claude-opus-4-6","tokens":{"input":10,"output":5}}"#,
            );
        }
        "overpowered_subagents_clean" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":10,"output":5}}"#,
            );
        }
        "overpowered_subagents_openai_finding" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "root-assistant",
                "root",
                20,
                r#"{"role":"assistant","modelID":"gpt-5.6-sol","tokens":{"input":10,"output":5}}"#,
            );
            insert_session(&connection, "child", Some("root"), 30);
            insert_message(
                &connection,
                "child-assistant",
                "child",
                40,
                r#"{"role":"assistant","modelID":"gpt-5.6-sol","tokens":{"input":10,"output":5}}"#,
            );
        }
        "overpowered_subagents_openai_luna_no_finding" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "root-assistant",
                "root",
                20,
                r#"{"role":"assistant","modelID":"gpt-5.6-sol","tokens":{"input":10,"output":5}}"#,
            );
            insert_session(&connection, "child", Some("root"), 30);
            insert_message(
                &connection,
                "child-assistant",
                "child",
                40,
                // GPT-5.6 Luna is not premium: only the Sol prefix is.
                r#"{"role":"assistant","modelID":"gpt-5.6-luna","tokens":{"input":10,"output":5}}"#,
            );
        }
        "excess_cache_rehydration_finding" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"claude-sonnet-4-6","tokens":{"input":40,"output":8,"cache":{"read":0,"write":9000}}}"#,
            );
            insert_message(
                &connection,
                "m2",
                "root",
                30,
                r#"{"role":"assistant","modelID":"claude-opus-4-6","tokens":{"input":30,"output":6,"cache":{"read":0,"write":8000}}}"#,
            );
        }
        "excess_cache_rehydration_clean" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":40,"output":8,"cache":{"read":0,"write":0}}}"#,
            );
        }
        "obsolete_model" => {
            insert_session(&connection, "root", None, 10);
            insert_message(
                &connection,
                "m1",
                "root",
                20,
                r#"{"role":"assistant","modelID":"model-a","tokens":{"input":40,"output":8}}"#,
            );
        }
        other => panic!("unknown opencode fixture: {other}"),
    }
    drop(connection);

    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Sqlite(path),
    };
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::opencode(),
    });
    let store = MemoryTurnRowStore::new(&input.agent, &input.session_id);
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        &input.session_id,
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream opencode source");
    sink.observe_source_outcome(outcome);
    sink.evidence().expect("evidence must publish")
}

struct Row {
    harness: &'static str,
    fixture: &'static str,
    badge: BadgeId,
    expected: BadgeStatus,
}

fn matrix() -> Vec<Row> {
    use BadgeId::*;
    use BadgeStatus::{Clean, Finding, NotAssessed};
    use NotAssessedReason::{
        CapabilityMissing, EvidenceContractIncomplete, IncompleteEvidence, SignalMissing,
    };

    vec![
        // ---------------- Claude ----------------
        Row {
            harness: "claude",
            fixture: "session_overdepth_finding",
            badge: SessionOverdepth,
            expected: Finding,
        },
        Row {
            harness: "claude",
            fixture: "multi_model_session",
            badge: SessionOverdepth,
            expected: Clean,
        },
        Row {
            harness: "claude",
            fixture: "malformed_between_valid",
            badge: SessionOverdepth,
            expected: NotAssessed(IncompleteEvidence),
        },
        Row {
            harness: "claude",
            fixture: "model_overthinking_finding",
            badge: ModelOverthinking,
            expected: Finding,
        },
        Row {
            harness: "claude",
            fixture: "reasoning_and_fast_mode",
            badge: ModelOverthinking,
            expected: Clean,
        },
        Row {
            // No turn in this fixture carries an effort value.
            // `present_turns` is zero, so the signal stays missing.
            harness: "claude",
            fixture: "multi_model_session",
            badge: ModelOverthinking,
            expected: NotAssessed(SignalMissing),
        },
        Row {
            // One of two eligible turns carries an effort value. It
            // shows no finding. Partial coverage now reads clean
            // instead of SignalMissing. A turn without the signal is
            // not negative evidence.
            harness: "claude",
            fixture: "model_overthinking_partial_coverage_clean",
            badge: ModelOverthinking,
            expected: Clean,
        },
        Row {
            harness: "claude",
            fixture: "delegated_models",
            badge: OverpoweredSubagents,
            expected: Finding,
        },
        Row {
            harness: "claude",
            fixture: "delegated_turns",
            badge: OverpoweredSubagents,
            expected: Clean,
        },
        Row {
            // A reviewed, non-empty registry with an uncatalogued
            // model reads clean: this fixture's model is not deprecated.
            harness: "claude",
            fixture: "records_all_kinds",
            badge: ObsoleteModel,
            expected: Clean,
        },
        Row {
            harness: "claude",
            fixture: "obsolete_model_finding",
            badge: ObsoleteModel,
            expected: Finding,
        },
        Row {
            // Usage of a since-replaced model, dated before the
            // replacement's effective date, is not a finding.
            harness: "claude",
            fixture: "obsolete_model_before_effective_date",
            badge: ObsoleteModel,
            expected: Clean,
        },
        Row {
            harness: "claude",
            fixture: "model_overthinking_unrecognized_tier",
            badge: ModelOverthinking,
            expected: NotAssessed(EvidenceContractIncomplete),
        },
        Row {
            harness: "claude",
            fixture: "reasoning_and_fast_mode",
            badge: FastModeOveruse,
            expected: Finding,
        },
        Row {
            harness: "claude",
            fixture: "fast_mode_overuse_clean",
            badge: FastModeOveruse,
            expected: Clean,
        },
        Row {
            // No turn in this fixture carries a speed value.
            // `present_turns` is zero, so the signal stays missing.
            harness: "claude",
            fixture: "multi_model_session",
            badge: FastModeOveruse,
            expected: NotAssessed(SignalMissing),
        },
        Row {
            // One of two eligible turns carries a speed value. It
            // shows no finding. Partial coverage now reads clean
            // instead of SignalMissing. A turn without the signal is
            // not negative evidence.
            harness: "claude",
            fixture: "fast_mode_overuse_partial_coverage_clean",
            badge: FastModeOveruse,
            expected: Clean,
        },
        Row {
            harness: "claude",
            fixture: "compaction_with_cache_rehydration",
            badge: ExcessCacheRehydration,
            expected: Finding,
        },
        Row {
            harness: "claude",
            fixture: "thread_identity_chain",
            badge: ExcessCacheRehydration,
            expected: Clean,
        },
        // ---------------- Codex ----------------
        Row {
            harness: "codex",
            fixture: "session_overdepth_finding",
            badge: SessionOverdepth,
            expected: Finding,
        },
        Row {
            harness: "codex",
            fixture: "records_all_kinds",
            badge: SessionOverdepth,
            expected: Clean,
        },
        Row {
            harness: "codex",
            fixture: "malformed_between_valid",
            badge: SessionOverdepth,
            expected: NotAssessed(IncompleteEvidence),
        },
        Row {
            harness: "codex",
            fixture: "model_overthinking_finding",
            badge: ModelOverthinking,
            expected: Finding,
        },
        Row {
            harness: "codex",
            fixture: "service_tier_priority",
            badge: ModelOverthinking,
            expected: Clean,
        },
        Row {
            harness: "codex",
            fixture: "absent_model_and_effort",
            badge: ModelOverthinking,
            expected: NotAssessed(SignalMissing),
        },
        // OverpoweredSubagents:Finding needs a genuine `scope = 'delegated'` turn.
        // For Codex that scope only comes from a discovered child rollout file
        // (apps/desktop's `discovery/agents/codex_subagents.rs`, joined via
        // `~/.codex/state_5.sqlite`'s `thread_spawn_edges`), which lives outside
        // this crate's `SessionInput -> composite sink` pipeline. Not
        // constructible here; the eligible/not-eligible split is unaffected
        // since `spawn_agent.jsonl` still keeps the detector eligible.
        Row {
            harness: "codex",
            fixture: "records_all_kinds",
            badge: OverpoweredSubagents,
            expected: Clean,
        },
        Row {
            // A reviewed, non-empty registry with an uncatalogued
            // model reads clean: this fixture's model is not deprecated.
            harness: "codex",
            fixture: "records_all_kinds",
            badge: ObsoleteModel,
            expected: Clean,
        },
        Row {
            harness: "codex",
            fixture: "obsolete_model_finding",
            badge: ObsoleteModel,
            expected: Finding,
        },
        // FastModeOveruse:Finding has the same `scope = 'delegated'` gap as
        // OverpoweredSubagents:Finding above.
        Row {
            harness: "codex",
            fixture: "service_tier_priority",
            badge: FastModeOveruse,
            expected: Clean,
        },
        Row {
            harness: "codex",
            fixture: "absent_model_and_effort",
            badge: FastModeOveruse,
            expected: NotAssessed(SignalMissing),
        },
        Row {
            // Codex has no `cache_write_tokens`, but `repeated_context`
            // resolves to uncached-input accounting from `token_classes`
            // and `request_context_tokens`, so the badge is eligible; it
            // never reads clean, because Codex has no `record_identity`
            // (the matrix's "Conditional using uncached-input accounting").
            // The unsupported `RecordLinkage` clean fact reports the
            // capability gap, not the coverage.
            harness: "codex",
            fixture: "records_all_kinds",
            badge: ExcessCacheRehydration,
            expected: NotAssessed(CapabilityMissing),
        },
        Row {
            harness: "codex",
            fixture: "context_reread",
            badge: ExcessCacheRehydration,
            expected: Finding,
        },
        // ---------------- Pi ----------------
        Row {
            harness: "pi",
            fixture: "session_overdepth_finding",
            badge: SessionOverdepth,
            expected: Finding,
        },
        Row {
            harness: "pi",
            fixture: "minimal_session",
            badge: SessionOverdepth,
            expected: Clean,
        },
        Row {
            harness: "pi",
            fixture: "malformed_middle",
            badge: SessionOverdepth,
            expected: NotAssessed(IncompleteEvidence),
        },
        Row {
            harness: "pi",
            fixture: "model_overthinking_finding",
            badge: ModelOverthinking,
            expected: Finding,
        },
        Row {
            harness: "pi",
            fixture: "thinking_level_change",
            badge: ModelOverthinking,
            expected: Clean,
        },
        Row {
            harness: "pi",
            fixture: "minimal_session",
            badge: ModelOverthinking,
            expected: NotAssessed(SignalMissing),
        },
        Row {
            harness: "pi",
            fixture: "minimal_session",
            badge: OverpoweredSubagents,
            expected: NotAssessed(CapabilityMissing),
        },
        Row {
            // A reviewed, non-empty registry with an uncatalogued
            // model reads clean: this fixture's model is not deprecated.
            harness: "pi",
            fixture: "minimal_session",
            badge: ObsoleteModel,
            expected: Clean,
        },
        Row {
            harness: "pi",
            fixture: "minimal_session",
            badge: FastModeOveruse,
            expected: NotAssessed(CapabilityMissing),
        },
        Row {
            harness: "pi",
            fixture: "excess_cache_rehydration_finding",
            badge: ExcessCacheRehydration,
            expected: Finding,
        },
        Row {
            harness: "pi",
            fixture: "minimal_session",
            badge: ExcessCacheRehydration,
            expected: Clean,
        },
        // ---------------- OpenCode ----------------
        Row {
            harness: "opencode",
            fixture: "session_overdepth_finding",
            badge: SessionOverdepth,
            expected: Finding,
        },
        Row {
            harness: "opencode",
            fixture: "session_overdepth_clean",
            badge: SessionOverdepth,
            expected: Clean,
        },
        Row {
            harness: "opencode",
            fixture: "session_overdepth_malformed",
            badge: SessionOverdepth,
            expected: NotAssessed(IncompleteEvidence),
        },
        Row {
            harness: "opencode",
            fixture: "model_overthinking_finding",
            badge: ModelOverthinking,
            expected: Finding,
        },
        Row {
            harness: "opencode",
            fixture: "model_overthinking_clean",
            badge: ModelOverthinking,
            expected: Clean,
        },
        Row {
            harness: "opencode",
            fixture: "model_overthinking_signal_missing",
            badge: ModelOverthinking,
            expected: NotAssessed(SignalMissing),
        },
        Row {
            harness: "opencode",
            fixture: "overpowered_subagents_finding",
            badge: OverpoweredSubagents,
            expected: Finding,
        },
        Row {
            harness: "opencode",
            fixture: "overpowered_subagents_clean",
            badge: OverpoweredSubagents,
            expected: Clean,
        },
        Row {
            harness: "opencode",
            fixture: "overpowered_subagents_openai_finding",
            badge: OverpoweredSubagents,
            expected: Finding,
        },
        Row {
            // GPT-5.6 Luna is not a premium model, so a Sol parent with
            // a Luna child is no finding.
            harness: "opencode",
            fixture: "overpowered_subagents_openai_luna_no_finding",
            badge: OverpoweredSubagents,
            expected: Clean,
        },
        Row {
            // A reviewed, non-empty registry with an uncatalogued
            // model reads clean: this fixture's model is not deprecated.
            harness: "opencode",
            fixture: "obsolete_model",
            badge: ObsoleteModel,
            expected: Clean,
        },
        Row {
            harness: "opencode",
            fixture: "obsolete_model",
            badge: FastModeOveruse,
            expected: NotAssessed(CapabilityMissing),
        },
        Row {
            harness: "opencode",
            fixture: "excess_cache_rehydration_finding",
            badge: ExcessCacheRehydration,
            expected: Finding,
        },
        Row {
            harness: "opencode",
            fixture: "excess_cache_rehydration_clean",
            badge: ExcessCacheRehydration,
            expected: Clean,
        },
    ]
}

#[test]
fn every_row_matches_the_real_pipeline() {
    let catalogs = ReportCatalogs::default();
    for row in matrix() {
        let evidence = match row.harness {
            "claude" => evidence_for(
                "claude",
                SourceCapabilities::claude(),
                row.fixture,
                claude_fixture(row.fixture),
            ),
            "codex" => evidence_for(
                "codex",
                SourceCapabilities::codex(),
                row.fixture,
                codex_fixture(row.fixture),
            ),
            "pi" => evidence_for(
                "pi",
                SourceCapabilities::pi(),
                row.fixture,
                pi_fixture(row.fixture),
            ),
            "opencode" => opencode_evidence(row.fixture),
            other => panic!("unknown harness: {other}"),
        };
        let badges = session_badges(&evidence, &catalogs);
        let actual = badges
            .iter()
            .find(|badge| badge.id == row.badge)
            .expect("BadgeId::ALL covers every badge")
            .status;
        assert_eq!(
            actual, row.expected,
            "{}/{} {:?}: expected {:?}, got {:?}",
            row.harness, row.fixture, row.badge, row.expected, actual
        );
    }
}

/// Maps one badge to the detector it reduces. This mirrors the private
/// mapping in `insights::badges::BadgeId::detector`. The mirror lets this
/// test call the public `eligible` and `clean_facts_complete` helpers,
/// which take a `DetectorId`, without exporting that private mapping.
fn badge_detector(id: BadgeId) -> DetectorId {
    match id {
        BadgeId::SessionOverdepth => DetectorId::SessionsOverDepth,
        BadgeId::ModelOverthinking => DetectorId::ModelOverthinking,
        BadgeId::OverpoweredSubagents => DetectorId::OverpoweredSubagents,
        BadgeId::ObsoleteModel => DetectorId::OldModelUsage,
        BadgeId::FastModeOveruse => DetectorId::OveruseOfFastMode,
        BadgeId::ExcessCacheRehydration => DetectorId::CacheChurn,
    }
}

/// Builds the real evidence for one matrix row's harness and fixture, the
/// same way `every_row_matches_the_real_pipeline` does.
fn evidence_for_row(harness: &str, fixture: &str) -> SessionEvidence {
    match harness {
        "claude" => evidence_for(
            "claude",
            SourceCapabilities::claude(),
            fixture,
            claude_fixture(fixture),
        ),
        "codex" => evidence_for(
            "codex",
            SourceCapabilities::codex(),
            fixture,
            codex_fixture(fixture),
        ),
        "pi" => evidence_for("pi", SourceCapabilities::pi(), fixture, pi_fixture(fixture)),
        "opencode" => opencode_evidence(fixture),
        other => panic!("unknown harness: {other}"),
    }
}

/// Sweeps every honesty invariant a badge status must respect, across
/// every harness and fixture the matrix already loads. These invariants
/// hold for every badge on every fixture, regardless of the matrix's
/// pinned expectation, so a future fixture or detector change cannot
/// smuggle in a dishonest state:
///
/// - Clean implies the detector is eligible, its clean facts are
///   complete, and the session's overall evidence coverage is complete.
/// - Finding implies the detector is eligible.
///
/// This intentionally does not assert the reverse: that an ineligible
/// detector never reports `NotAssessed(CapabilityMissing)` while every
/// finding fact and every clean fact is supported. That reverse mapping
/// is under active change; the two implications above hold on both
/// sides of that change.
#[test]
fn every_badge_status_respects_the_honesty_invariants() {
    let catalogs = ReportCatalogs::default();
    let mut swept = std::collections::BTreeSet::new();
    for row in matrix() {
        if !swept.insert((row.harness, row.fixture)) {
            continue;
        }
        let evidence = evidence_for_row(row.harness, row.fixture);
        for badge in session_badges(&evidence, &catalogs) {
            let detector = badge_detector(badge.id);
            match badge.status {
                BadgeStatus::Clean => {
                    assert!(
                        eligible(detector, &evidence),
                        "{}/{} {:?}: Clean but the detector is not eligible",
                        row.harness,
                        row.fixture,
                        badge.id
                    );
                    assert!(
                        clean_facts_complete(detector, &evidence),
                        "{}/{} {:?}: Clean but clean facts are not complete",
                        row.harness,
                        row.fixture,
                        badge.id
                    );
                    assert_eq!(
                        evidence.coverage,
                        EvidenceCoverage::Complete,
                        "{}/{} {:?}: Clean but session evidence coverage is not complete",
                        row.harness,
                        row.fixture,
                        badge.id
                    );
                }
                BadgeStatus::Finding => {
                    assert!(
                        eligible(detector, &evidence),
                        "{}/{} {:?}: Finding but the detector is not eligible",
                        row.harness,
                        row.fixture,
                        badge.id
                    );
                }
                BadgeStatus::NotAssessed(_) => {}
            }
        }
    }
}
