//! Assembly check between `query_turn_facts` (the row-derived read side)
//! and `SessionEvidenceAccumulator::evidence` (the published projection).
//! See Phase 3 in `docs/plans/session-evidence-harness-parity.md`.
//!
//! `SessionEvidence` is now built directly from `TurnFacts`, so this is a
//! cheap check that the assembly carries every row-derived field through
//! unchanged, not a comparison between two independent computations. Each
//! vendor test streams every characterization fixture once, through a
//! `CompositeSink` fanned out to a `MemoryTurnRowStore`, then asserts the
//! published `SessionEvidence` groups equal the queried `TurnFacts` values
//! field by field, for every fixture, with no exceptions.
//!
//! Row-derived fields still follow three rules, now built into
//! `query_turn_facts` and no longer worth a per-fixture allowlist:
//!
//! - `time_range` spans turn rows only. An eventless record (a
//!   `RecordTimestamp` observation with no turn behind it) never moves it.
//! - `delegated_turns` counts delegated turn rows only. An inert
//!   sidechain record never becomes a row, so it never counts.
//! - `model_transitions` and idle gaps use `scope='main'` rows only, per
//!   thread. A sidechain turn never forms a transition or a gap with a
//!   main-loop turn.

use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, EvidenceValue, MemoryTurnRowStore, RawSource, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceCapabilities,
    SourceKind, TurnFacts, TurnRowSink, TurnRowStore, adapter_for,
};

/// Reads the value out of an `EvidenceValue`, for `Complete` and `Partial`
/// alike. Returns `None` for `Unsupported`, so the caller skips a group the
/// vendor never publishes.
fn published<T: Clone>(value: &EvidenceValue<T>) -> Option<T> {
    match value {
        EvidenceValue::Complete(observed) => Some(observed.clone()),
        EvidenceValue::Partial { observed, .. } => Some(observed.clone()),
        EvidenceValue::Unsupported => None,
    }
}

/// Records a mismatch when the published `SessionEvidence` group and the
/// queried `TurnFacts` differ for one field of one fixture.
fn diff<T: std::fmt::Debug + PartialEq>(
    mismatches: &mut Vec<Mismatch>,
    fixture: &str,
    field: &str,
    evidence_value: T,
    facts: T,
) {
    if evidence_value != facts {
        mismatches.push(Mismatch {
            detail: format!(
                "fixture={fixture} field={field}\n  evidence = {evidence_value:?}\n  facts    = {facts:?}"
            ),
        });
    }
}

struct Mismatch {
    detail: String,
}

/// Streams `jsonl` through the named vendor's adapter into both an evidence
/// accumulator and a `MemoryTurnRowStore`, and returns both projections.
fn run_fixture(
    agent: &str,
    fixture: &str,
    jsonl: &str,
    capabilities: SourceCapabilities,
) -> (SessionEvidence, TurnFacts) {
    let input = SessionInput {
        agent: agent.to_owned(),
        session_id: fixture.to_owned(),
        source: RawSource::Jsonl(jsonl.to_owned()),
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
    let mut composite = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = adapter_for(agent)
        .visit(&input, &mut composite)
        .expect("fixture must stream");
    composite.observe_source_outcome(outcome);
    assert!(
        !composite.turn_row_write_failed(),
        "fixture {fixture}: turn row write must not fail"
    );

    let evidence = composite
        .evidence()
        .expect("finished source must publish evidence");
    let facts = store.query_turn_facts().expect("facts query must succeed");
    (evidence, facts)
}

/// Compares every group both projections produce, for one fixture, pushing
/// each field mismatch found into `mismatches`.
fn compare_fixture(
    mismatches: &mut Vec<Mismatch>,
    fixture: &str,
    evidence: &SessionEvidence,
    facts: &TurnFacts,
) {
    if let Some(eligibility) = published(&evidence.eligibility) {
        diff(
            mismatches,
            fixture,
            "eligibility",
            eligibility,
            facts.eligibility.clone(),
        );
    }
    if let Some(context) = published(&evidence.context) {
        diff(
            mismatches,
            fixture,
            "context.maxRequestContextTokens",
            context.max_request_context_tokens,
            facts.max_request_context_tokens,
        );
        diff(
            mismatches,
            fixture,
            "context.topDepthExamples",
            context.top_depth_examples,
            facts.top_depth_examples.clone(),
        );
    }
    if let Some(time_range) = published(&evidence.time_range) {
        diff(
            mismatches,
            fixture,
            "timeRange",
            time_range,
            facts.time_range.clone(),
        );
    }
    if let Some(models) = published(&evidence.models) {
        diff(
            mismatches,
            fixture,
            "models.byModel",
            models.by_model,
            facts.by_model.clone(),
        );
        diff(
            mismatches,
            fixture,
            "models.unattributedTurns",
            models.unattributed_turns,
            facts.unattributed_turns,
        );
        diff(
            mismatches,
            fixture,
            "models.effortTiers",
            models.effort_tiers,
            facts.effort_tiers.clone(),
        );
        diff(
            mismatches,
            fixture,
            "models.fastModes",
            models.fast_modes,
            facts.fast_modes.clone(),
        );
        diff(
            mismatches,
            fixture,
            "models.effortSignal",
            models.effort_signal,
            facts.effort_signal,
        );
        diff(
            mismatches,
            fixture,
            "models.speedSignal",
            models.speed_signal,
            facts.speed_signal,
        );
    }
    if let Some(subagents) = published(&evidence.subagents) {
        diff(
            mismatches,
            fixture,
            "subagents.delegatedTurns",
            subagents.delegated_turns,
            facts.delegated_turns,
        );
        diff(
            mismatches,
            fixture,
            "subagents.delegatedModels",
            subagents.delegated_models,
            facts.delegated_models.clone(),
        );
    }
    if let Some(cache) = published(&evidence.cache) {
        diff(
            mismatches,
            fixture,
            "cache.cacheReadTokens",
            cache.cache_read_tokens,
            facts.cache_read_tokens,
        );
        diff(
            mismatches,
            fixture,
            "cache.cacheCreationTokens",
            cache.cache_creation_tokens,
            facts.cache_creation_tokens,
        );
        diff(
            mismatches,
            fixture,
            "cache.freshInputTokens",
            cache.fresh_input_tokens,
            facts.fresh_input_tokens,
        );
        diff(
            mismatches,
            fixture,
            "cache.modelTransitions",
            cache.model_transitions,
            facts.model_transitions.clone(),
        );
        diff(
            mismatches,
            fixture,
            "cache.longestIdleGapMs",
            cache.longest_idle_gap_ms,
            facts.longest_idle_gap_ms,
        );
        diff(
            mismatches,
            fixture,
            "cache.idleGapMsTotal",
            cache.idle_gap_ms_total,
            facts.idle_gap_ms_total,
        );
        diff(
            mismatches,
            fixture,
            "cache.userControlledChurn.manualCompactions",
            cache.user_controlled_churn.manual_compactions,
            facts.manual_compactions,
        );
    }
    if let Some(compactions) = published(&evidence.compactions) {
        diff(
            mismatches,
            fixture,
            "compactions.boundaries",
            compactions.boundaries,
            facts.compaction_boundaries.clone(),
        );
    }
}

/// Asserts every fixture's published `SessionEvidence` groups carry the
/// queried `TurnFacts` values through with no change. `evidence()` builds
/// each group straight from `facts`, so any mismatch here is a bug in that
/// assembly, not a semantic difference to document.
fn assert_no_mismatches(vendor: &str, mismatches: Vec<Mismatch>) {
    let details: Vec<&str> = mismatches
        .iter()
        .map(|mismatch| mismatch.detail.as_str())
        .collect();
    assert!(
        details.is_empty(),
        "{vendor}: {} mismatch(es) between the published SessionEvidence and query_turn_facts:\n\n{}",
        details.len(),
        details.join("\n\n")
    );
}

/* --------------------------------------------------------------------
 * Claude fixtures. Reuses the fixture set `evidence_fixture_names`
 * exercises in `claude_characterization.rs` — every fixture that suite
 * treats as evidence-bearing.
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
fn claude_facts_match_evidence_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in claude_fixture_names() {
        let (evidence, facts) = run_fixture(
            "claude",
            name,
            claude_fixture(name),
            SourceCapabilities::claude(),
        );
        compare_fixture(&mut mismatches, name, &evidence, &facts);
    }
    assert_no_mismatches("claude", mismatches);
}

/* --------------------------------------------------------------------
 * Codex fixtures. Reuses the fixture set `codex_characterization.rs`
 * streams through `composite`.
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
fn codex_facts_match_evidence_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in codex_fixture_names() {
        let (evidence, facts) = run_fixture(
            "codex",
            name,
            codex_fixture(name),
            SourceCapabilities::codex(),
        );
        compare_fixture(&mut mismatches, name, &evidence, &facts);
    }
    assert_no_mismatches("codex", mismatches);
}

/* --------------------------------------------------------------------
 * Pi fixtures. Reuses the fixture set `pi_characterization.rs` streams
 * through `composite`.
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
fn pi_facts_match_evidence_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in pi_fixture_names() {
        let (evidence, facts) = run_fixture("pi", name, pi_fixture(name), SourceCapabilities::pi());
        compare_fixture(&mut mismatches, name, &evidence, &facts);
    }
    assert_no_mismatches("pi", mismatches);
}
