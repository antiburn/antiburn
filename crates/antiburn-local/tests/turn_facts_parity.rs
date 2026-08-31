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

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, EvidenceValue, MemoryTurnRowStore, ModelRun, RawSource,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetrics,
    SessionMetricsAccumulator, SourceCapabilities, SourceKind, TurnFacts, TurnRowSink,
    TurnRowStore, adapter_for,
};
use antiburn_local::pricing::ModelTokens;
use rusqlite::{Connection, params};
use tempfile::TempDir;

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
    let (evidence, facts, _, _, _) = run_fixture_with_row_projections(
        agent,
        fixture,
        &SessionInput {
            agent: agent.to_owned(),
            session_id: fixture.to_owned(),
            source: RawSource::Jsonl(jsonl.to_owned()),
        },
        capabilities,
    );
    (evidence, facts)
}

/// Like [`run_fixture`], but also returns the accumulator's own
/// `SessionMetrics` alongside the row-derived `query_model_breakdown`/
/// `query_model_runs` projections the seam R2 parity tests
/// (`model_breakdown_and_model_runs_match_the_accumulator_for_every_fixture`)
/// compare against it. Takes a built `SessionInput` rather than raw
/// `jsonl` so the OpenCode fixtures below, which build a `SessionInput`
/// straight from a database, share this same streaming setup.
fn run_fixture_with_row_projections(
    agent: &str,
    fixture: &str,
    input: &SessionInput,
    capabilities: SourceCapabilities,
) -> (
    SessionEvidence,
    TurnFacts,
    SessionMetrics,
    BTreeMap<String, ModelTokens>,
    Vec<ModelRun>,
) {
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

    let evidence = composite
        .evidence()
        .expect("finished source must publish evidence");
    let session_metrics = composite
        .metrics()
        .expect("finished source must publish metrics");
    let facts = store.query_turn_facts().expect("facts query must succeed");
    let model_breakdown = store
        .query_model_breakdown()
        .expect("model breakdown query must succeed");
    let model_runs = store
        .query_model_runs()
        .expect("model runs query must succeed");
    (
        evidence,
        facts,
        session_metrics,
        model_breakdown,
        model_runs,
    )
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

/// Seam R2: `query_model_breakdown` and `query_model_runs`
/// (`analysis/evidence_query.rs`) must equal what
/// `SessionMetricsAccumulator` itself computed for this fixture —
/// `metrics.model_breakdown` and, once normalized the same way
/// `apps/desktop/src-tauri/src/analysis.rs`'s `model_runs_for_metrics`
/// does, `metrics.model_runs`. Every fixture here streams through one
/// `CompositeSink` with no sub-agent transcript of its own, so there is
/// no parent/child file split for `query_model_runs`'s merge rule to
/// exercise — every row's `source_key` equals the session id, so the
/// query's own "parent" bucket already holds every row. The OpenCode
/// `subagent_delegation_with_model_transition` fixture below is the
/// exception: its child session's rows share the parent's `source_key`
/// too (one `SessionInput` reads the whole OpenCode session tree), so it
/// exercises the same flat-set shape, not the multi-file merge — that
/// merge is desktop-only (`model_runs_parent_first`), assembled from
/// several passes' own metrics, and has no engine-level equivalent to
/// compare against.
fn compare_model_projections(
    mismatches: &mut Vec<Mismatch>,
    fixture: &str,
    metrics: &SessionMetrics,
    model_breakdown: &BTreeMap<String, ModelTokens>,
    model_runs: &[ModelRun],
) {
    let expected_breakdown: BTreeMap<String, ModelTokens> = metrics
        .model_breakdown
        .iter()
        .map(|(model, tokens)| (model.clone(), tokens.clone()))
        .collect();
    diff(
        mismatches,
        fixture,
        "model_breakdown",
        model_breakdown.clone(),
        expected_breakdown,
    );
    diff(
        mismatches,
        fixture,
        "model_runs",
        model_runs.to_vec(),
        expected_model_runs(metrics),
    );
}

/// Mirrors `apps/desktop/src-tauri/src/analysis.rs`'s
/// `model_runs_for_metrics`: trims each run's model and thinking mode,
/// drops a run whose model is empty after trimming, then collects into a
/// `BTreeSet` so only the set of distinct pairs decides the result, not
/// the accumulator's mark order. Falls back to one run per breakdown
/// model, with no thinking mode, when the accumulator recorded no mark at
/// all.
fn expected_model_runs(metrics: &SessionMetrics) -> Vec<ModelRun> {
    if metrics.model_runs.is_empty() {
        let mut models: Vec<String> = metrics.model_breakdown.keys().cloned().collect();
        models.sort();
        return models
            .into_iter()
            .map(|model| ModelRun {
                model,
                thinking_mode: None,
            })
            .collect();
    }
    metrics
        .model_runs
        .iter()
        .filter_map(|run| {
            let model = run.model.trim();
            if model.is_empty() {
                return None;
            }
            let thinking_mode = run
                .thinking_mode
                .as_deref()
                .map(str::trim)
                .filter(|mode| !mode.is_empty())
                .map(str::to_string);
            Some(ModelRun {
                model: model.to_string(),
                thinking_mode,
            })
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
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

#[test]
fn claude_model_projections_match_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in claude_fixture_names() {
        let input = SessionInput {
            agent: "claude".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(claude_fixture(name).to_owned()),
        };
        let (_, _, metrics, model_breakdown, model_runs) =
            run_fixture_with_row_projections("claude", name, &input, SourceCapabilities::claude());
        // `delegated_model_missing` carries an assistant turn with billable
        // tokens and no model. `SessionMetricsAccumulator` folds such a
        // turn's tokens into the transcript's own primary model
        // (`summary.model`, the parent's `claude-opus-4-6`) — see
        // `model_breakdown_map`'s `unattributed_model_tokens` fold in
        // `metrics_sink/mod.rs`. `query_model_breakdown` filters to
        // `model IS NOT NULL` rows only, per this seam's design, and reads
        // no session-level primary model to fold an unattributed turn's
        // tokens into instead, so its total for `claude-opus-4-6` on this
        // one fixture is short by that turn's 12 input / 3 output tokens.
        // This is the one documented parity gap this seam accepts — see
        // the PR description — so `model_breakdown` is excluded from this
        // fixture's comparison; `model_runs` still holds, since the
        // model-less turn's resolved run (`claude-opus-4-6`, same effort)
        // duplicates one the modeled turn already contributes.
        if name == "delegated_model_missing" {
            diff(
                &mut mismatches,
                name,
                "model_runs",
                model_runs,
                expected_model_runs(&metrics),
            );
            continue;
        }
        compare_model_projections(
            &mut mismatches,
            name,
            &metrics,
            &model_breakdown,
            &model_runs,
        );
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

#[test]
fn codex_model_projections_match_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in codex_fixture_names() {
        let input = SessionInput {
            agent: "codex".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(codex_fixture(name).to_owned()),
        };
        let (_, _, metrics, model_breakdown, model_runs) =
            run_fixture_with_row_projections("codex", name, &input, SourceCapabilities::codex());
        compare_model_projections(
            &mut mismatches,
            name,
            &metrics,
            &model_breakdown,
            &model_runs,
        );
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

#[test]
fn pi_model_projections_match_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();
    for name in pi_fixture_names() {
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: name.to_owned(),
            source: RawSource::Jsonl(pi_fixture(name).to_owned()),
        };
        let (_, _, metrics, model_breakdown, model_runs) =
            run_fixture_with_row_projections("pi", name, &input, SourceCapabilities::pi());
        compare_model_projections(
            &mut mismatches,
            name,
            &metrics,
            &model_breakdown,
            &model_runs,
        );
    }
    assert_no_mismatches("pi", mismatches);
}

/* --------------------------------------------------------------------
 * OpenCode fixtures. OpenCode has no `fixtures/opencode_characterization`
 * directory: `opencode_characterization.rs` builds every session inline,
 * either from an in-memory SQLite database or from an inline export-JSONL
 * string. This section mirrors that suite's own `create_database` /
 * `insert_session` / `insert_message` / `insert_part` helpers, and copies
 * the minimal synthetic content of two of its scenarios, in miniature
 * here, so `opencode_characterization.rs` itself stays untouched. The
 * content is synthetic test fixture data, so this duplication is
 * acceptable.
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
    }
}

/// Streams one OpenCode `SessionInput` through the real adapter and the
/// real evidence and turn-row pipeline, the same path production uses.
fn run_opencode_fixture(fixture: &str, input: &SessionInput) -> (SessionEvidence, TurnFacts) {
    let (evidence, facts, _, _, _) = run_fixture_with_row_projections(
        "opencode",
        fixture,
        input,
        SourceCapabilities::opencode(),
    );
    (evidence, facts)
}

/// A root session with one assistant message carrying a model, an effort
/// variant, and cache read/write and reasoning tokens. Exercises the
/// `models` and `cache` evidence groups.
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

/// A root session with two assistant messages under different models
/// (a model transition) and a delegated child session with its own
/// model. Exercises `subagents`, `models.byModel`, and the cache group's
/// model-transition and idle-gap fields.
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

/// A malformed message row between two valid ones. Exercises the
/// `eligibility` group and the partial-coverage path every other group
/// carries when the session-wide claim is incomplete.
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

/// One assistant message that carries a compaction part. Exercises the
/// `compactions` evidence group.
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

/// The export-JSONL format (`session_meta` / `session_member` / `message`
/// records), copied from `opencode_characterization.rs`'s
/// `export_stream_marks_a_child_message_as_delegated_with_one_spawn`.
/// Exercises the JSONL source path, distinct from the SQLite path every
/// other OpenCode fixture in this file uses.
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
    }
}

#[test]
fn opencode_facts_match_evidence_for_every_fixture() {
    let mut mismatches = Vec::new();

    let (_messages_dir, messages_input) = opencode_fixture_messages_with_cache_and_reasoning();
    let (evidence, facts) =
        run_opencode_fixture("messages_with_cache_and_reasoning", &messages_input);
    compare_fixture(
        &mut mismatches,
        "messages_with_cache_and_reasoning",
        &evidence,
        &facts,
    );

    let (_subagent_dir, subagent_input) =
        opencode_fixture_subagent_delegation_with_model_transition();
    let (evidence, facts) =
        run_opencode_fixture("subagent_delegation_with_model_transition", &subagent_input);
    compare_fixture(
        &mut mismatches,
        "subagent_delegation_with_model_transition",
        &evidence,
        &facts,
    );

    let (_malformed_dir, malformed_input) = opencode_fixture_malformed_between_valid();
    let (evidence, facts) = run_opencode_fixture("malformed_between_valid", &malformed_input);
    compare_fixture(
        &mut mismatches,
        "malformed_between_valid",
        &evidence,
        &facts,
    );

    let (_compaction_dir, compaction_input) = opencode_fixture_compaction_boundary();
    let (evidence, facts) = run_opencode_fixture("compaction_boundary", &compaction_input);
    compare_fixture(&mut mismatches, "compaction_boundary", &evidence, &facts);

    let export_input = opencode_fixture_export_jsonl_child_delegation();
    let (evidence, facts) = run_opencode_fixture("export_jsonl_child_delegation", &export_input);
    compare_fixture(
        &mut mismatches,
        "export_jsonl_child_delegation",
        &evidence,
        &facts,
    );

    assert_no_mismatches("opencode", mismatches);
}

#[test]
fn opencode_model_projections_match_the_accumulator_for_every_fixture() {
    let mut mismatches = Vec::new();

    let (_messages_dir, messages_input) = opencode_fixture_messages_with_cache_and_reasoning();
    let (_, _, metrics, model_breakdown, model_runs) = run_fixture_with_row_projections(
        "opencode",
        "messages_with_cache_and_reasoning",
        &messages_input,
        SourceCapabilities::opencode(),
    );
    compare_model_projections(
        &mut mismatches,
        "messages_with_cache_and_reasoning",
        &metrics,
        &model_breakdown,
        &model_runs,
    );

    let (_subagent_dir, subagent_input) =
        opencode_fixture_subagent_delegation_with_model_transition();
    let (_, _, metrics, model_breakdown, model_runs) = run_fixture_with_row_projections(
        "opencode",
        "subagent_delegation_with_model_transition",
        &subagent_input,
        SourceCapabilities::opencode(),
    );
    compare_model_projections(
        &mut mismatches,
        "subagent_delegation_with_model_transition",
        &metrics,
        &model_breakdown,
        &model_runs,
    );

    let (_malformed_dir, malformed_input) = opencode_fixture_malformed_between_valid();
    let (_, _, metrics, model_breakdown, model_runs) = run_fixture_with_row_projections(
        "opencode",
        "malformed_between_valid",
        &malformed_input,
        SourceCapabilities::opencode(),
    );
    compare_model_projections(
        &mut mismatches,
        "malformed_between_valid",
        &metrics,
        &model_breakdown,
        &model_runs,
    );

    let (_compaction_dir, compaction_input) = opencode_fixture_compaction_boundary();
    let (_, _, metrics, model_breakdown, model_runs) = run_fixture_with_row_projections(
        "opencode",
        "compaction_boundary",
        &compaction_input,
        SourceCapabilities::opencode(),
    );
    compare_model_projections(
        &mut mismatches,
        "compaction_boundary",
        &metrics,
        &model_breakdown,
        &model_runs,
    );

    let export_input = opencode_fixture_export_jsonl_child_delegation();
    let (_, _, metrics, model_breakdown, model_runs) = run_fixture_with_row_projections(
        "opencode",
        "export_jsonl_child_delegation",
        &export_input,
        SourceCapabilities::opencode(),
    );
    compare_model_projections(
        &mut mismatches,
        "export_jsonl_child_delegation",
        &metrics,
        &model_breakdown,
        &model_runs,
    );

    assert_no_mismatches("opencode", mismatches);
}
