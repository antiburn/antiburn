//! Row-derived evidence facts: the read side of persisted turn rows.
//!
//! [`query_turn_facts`] reads the facts a later change uses to compute most
//! of `SessionEvidence`, straight from the `turn` rows a pass already wrote
//! — instead of by streaming an accumulator over the transcript again. See
//! `docs/plans/session-evidence-harness-parity.md`.
//!
//! Every query here filters on the same four columns: `environment_key`,
//! `agent`, `session_id`, and `claim_fence`.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};

use crate::analysis::evidence::{
    CompactionBoundary, DepthExample, EligibilityEvidence, MAX_COMPACTION_BOUNDARIES,
    MAX_EVIDENCE_EXAMPLES, MAX_MODEL_TRANSITIONS, MAX_MODELS, MAX_SUBAGENT_MODELS, MAX_TIER_LABELS,
    ModelTokens, ModelTransition, ParseDiagnostics, SessionTimeRange, SignalCoverage, TurnCounts,
    cap_string, insert_diagnostic_field, record_diagnostic_set_cap,
};
use crate::analysis::model::CompactionTrigger;
use crate::analysis::rows::TurnSessionKey;

/// The row-derived facts for one session, at one claim fence.
///
/// A later change computes most of `SessionEvidence` from these instead of
/// from `SessionEvidenceAccumulator`. The field-by-field rule each fact
/// follows is documented next to the query that computes it, below.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnFacts {
    pub eligibility: EligibilityEvidence,
    pub max_request_context_tokens: u64,
    pub top_depth_examples: Vec<DepthExample>,
    pub depth_examples_capped: bool,
    pub time_range: SessionTimeRange,
    pub by_model: BTreeMap<String, ModelTokens>,
    pub models_capped: bool,
    pub dominant_main_model: Option<String>,
    pub unattributed_turns: u64,
    pub effort_tiers: BTreeMap<String, TurnCounts>,
    pub fast_modes: BTreeMap<String, TurnCounts>,
    pub tiers_capped: bool,
    pub effort_signal: SignalCoverage,
    pub speed_signal: SignalCoverage,
    pub delegated_turns: u64,
    pub delegated_models: BTreeSet<String>,
    pub delegated_models_capped: bool,
    pub delegated_model_missing: bool,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub fresh_input_tokens: u64,
    pub model_transitions: Vec<ModelTransition>,
    pub transitions_capped: bool,
    pub longest_idle_gap_ms: i64,
    pub idle_gap_ms_total: i64,
    pub manual_compactions: u64,
    pub compaction_boundaries: Vec<CompactionBoundary>,
    pub compactions_capped: bool,
    pub duplicate_turn_identities: u64,
    pub thread_identity_missing: bool,
    /// Repeated-context accounting under cache-write billing: paid context
    /// (`cache_write_tokens`) beyond positive growth, summed over adjacent
    /// `scope = 'main'` assistant turn pairs within one thread.
    pub repeated_context_cache_write_tokens: u64,
    /// The same sum under uncached-input billing: paid context
    /// (`input_tokens`) beyond positive growth.
    pub repeated_context_uncached_input_tokens: u64,
    pub repeated_context_pairs_considered: u64,
    /// Adjacent pairs skipped for a missing or non-monotonic `ts_ms`.
    pub repeated_context_pairs_skipped: u64,
    pub diagnostics: ParseDiagnostics,
}

/// Reads every [`TurnFacts`] field for the rows `key` and `claim_fence`
/// select.
pub fn query_turn_facts(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<TurnFacts> {
    let mut diagnostics = ParseDiagnostics::new();

    let core = query_core(conn, key, claim_fence)?;
    let (top_depth_examples, depth_examples_capped) =
        query_top_depth_examples(conn, key, claim_fence, &mut diagnostics)?;
    let (by_model, models_capped) = query_by_model(conn, key, claim_fence, &mut diagnostics)?;
    let dominant_main_model = query_dominant_main_model(conn, key, claim_fence, &mut diagnostics)?;
    let (effort_tiers, effort_capped) = query_tier_map(
        conn,
        key,
        claim_fence,
        EFFORT_TIERS_SQL,
        "models.effort_tiers",
        &mut diagnostics,
    )?;
    let (fast_modes, fast_capped) = query_tier_map(
        conn,
        key,
        claim_fence,
        FAST_MODES_SQL,
        "models.fast_modes",
        &mut diagnostics,
    )?;
    let (effort_signal, speed_signal) = query_signal_coverage(conn, key, claim_fence)?;
    let (delegated_models, delegated_models_capped) =
        query_delegated_models(conn, key, claim_fence, &mut diagnostics)?;
    let (model_transitions, transitions_capped, longest_idle_gap_ms, idle_gap_ms_total) =
        query_transitions_and_idle_gaps(conn, key, claim_fence, &mut diagnostics)?;
    let manual_compactions = query_manual_compactions(conn, key, claim_fence)?;
    let (compaction_boundaries, compactions_capped) =
        query_compaction_boundaries(conn, key, claim_fence, &mut diagnostics)?;
    let duplicate_turn_identities = query_duplicate_turn_identities(conn, key, claim_fence)?;
    let repeated_context = query_repeated_context(conn, key, claim_fence)?;

    Ok(TurnFacts {
        eligibility: core.eligibility,
        max_request_context_tokens: core.max_request_context_tokens,
        top_depth_examples,
        depth_examples_capped,
        time_range: core.time_range,
        by_model,
        models_capped,
        dominant_main_model,
        unattributed_turns: core.unattributed_turns,
        effort_tiers,
        fast_modes,
        tiers_capped: effort_capped || fast_capped,
        effort_signal,
        speed_signal,
        delegated_turns: core.delegated_turns,
        delegated_models,
        delegated_models_capped,
        delegated_model_missing: core.delegated_model_missing,
        cache_read_tokens: core.cache_read_tokens,
        cache_creation_tokens: core.cache_creation_tokens,
        fresh_input_tokens: core.fresh_input_tokens,
        model_transitions,
        transitions_capped,
        longest_idle_gap_ms,
        idle_gap_ms_total,
        manual_compactions,
        compaction_boundaries,
        compactions_capped,
        duplicate_turn_identities,
        thread_identity_missing: core.thread_identity_missing,
        repeated_context_cache_write_tokens: repeated_context.cache_write_tokens,
        repeated_context_uncached_input_tokens: repeated_context.uncached_input_tokens,
        repeated_context_pairs_considered: repeated_context.pairs_considered,
        repeated_context_pairs_skipped: repeated_context.pairs_skipped,
        diagnostics,
    })
}

/// Clamps a SQLite `INTEGER` aggregate to a non-negative token or turn
/// count. Every source column this module reads is a non-negative count,
/// so a negative aggregate can only mean an empty `SUM`, which SQLite
/// already reports as `0` through `COALESCE`; the clamp is a second
/// guard, not the primary defense.
fn as_u64(value: i64) -> u64 {
    value.max(0) as u64
}

/// Records that one bounded collection overflowed, the same way
/// `SessionEvidenceAccumulator::note_collection_cap` does.
fn note_collection_cap(diagnostics: &mut ParseDiagnostics, field: &'static str) {
    if insert_diagnostic_field(&mut diagnostics.capped_collections, field) {
        record_diagnostic_set_cap(diagnostics, "diagnostics.capped_collections");
    }
}

/* --------------------------------------------------------------------
 * Core aggregate: eligibility, context depth, time range, cache sums,
 * unattributed turns, delegated turns, and the two identity flags.
 * ----------------------------------------------------------------- */

struct Core {
    eligibility: EligibilityEvidence,
    max_request_context_tokens: u64,
    time_range: SessionTimeRange,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    fresh_input_tokens: u64,
    unattributed_turns: u64,
    delegated_turns: u64,
    delegated_model_missing: bool,
    thread_identity_missing: bool,
}

/// "Depth" is `input_tokens + cache_read_tokens + cache_write_tokens` —
/// this equals `Usage::context_tokens()`.
const CORE_SQL: &str = "SELECT
    COUNT(*),
    COALESCE(SUM(role = 'assistant'), 0),
    COALESCE(SUM(role = 'tool'), 0),
    COALESCE(SUM((input_tokens + cache_read_tokens + cache_write_tokens) > 0), 0),
    COALESCE(MAX(input_tokens + cache_read_tokens + cache_write_tokens), 0),
    COALESCE(MIN(ts_ms), 0),
    COALESCE(MAX(ts_ms), 0),
    COALESCE(SUM(ts_ms IS NOT NULL), 0),
    COALESCE(SUM(cache_read_tokens), 0),
    COALESCE(SUM(cache_write_tokens), 0),
    COALESCE(SUM(input_tokens), 0),
    COALESCE(SUM(role = 'assistant' AND model IS NULL), 0),
    COALESCE(SUM(scope = 'delegated'), 0),
    COALESCE(SUM(scope = 'delegated' AND role = 'assistant' AND model IS NULL), 0),
    COALESCE(SUM(uuid IS NULL), 0)
  FROM turn
 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4";

fn query_core(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<Core> {
    conn.query_row(
        CORE_SQL,
        params![key.environment_key, key.agent, key.session_id, claim_fence],
        |row| {
            let turns: i64 = row.get(0)?;
            let assistant_turns: i64 = row.get(1)?;
            let tool_turns: i64 = row.get(2)?;
            let depth_eligible_turns: i64 = row.get(3)?;
            let max_request_context_tokens: i64 = row.get(4)?;
            let first_ts_ms: i64 = row.get(5)?;
            let last_ts_ms: i64 = row.get(6)?;
            let timestamped_turns: i64 = row.get(7)?;
            let cache_read_tokens: i64 = row.get(8)?;
            let cache_creation_tokens: i64 = row.get(9)?;
            let fresh_input_tokens: i64 = row.get(10)?;
            let unattributed_turns: i64 = row.get(11)?;
            let delegated_turns: i64 = row.get(12)?;
            let delegated_model_missing: i64 = row.get(13)?;
            let thread_identity_missing: i64 = row.get(14)?;
            Ok(Core {
                eligibility: EligibilityEvidence {
                    turns: as_u64(turns),
                    assistant_turns: as_u64(assistant_turns),
                    tool_turns: as_u64(tool_turns),
                    depth_eligible_turns: as_u64(depth_eligible_turns),
                },
                max_request_context_tokens: as_u64(max_request_context_tokens),
                time_range: SessionTimeRange {
                    first_ts_ms,
                    last_ts_ms,
                    timestamped_turns: as_u64(timestamped_turns),
                },
                cache_read_tokens: as_u64(cache_read_tokens),
                cache_creation_tokens: as_u64(cache_creation_tokens),
                fresh_input_tokens: as_u64(fresh_input_tokens),
                unattributed_turns: as_u64(unattributed_turns),
                delegated_turns: as_u64(delegated_turns),
                delegated_model_missing: delegated_model_missing > 0,
                thread_identity_missing: thread_identity_missing > 0,
            })
        },
    )
}

/* --------------------------------------------------------------------
 * Top depth examples.
 * ----------------------------------------------------------------- */

const TOP_DEPTH_EXAMPLES_SQL: &str = "SELECT ts_ms,
        (input_tokens + cache_read_tokens + cache_write_tokens) AS depth, model
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND ts_ms IS NOT NULL
    AND (input_tokens + cache_read_tokens + cache_write_tokens) > 0
  ORDER BY depth DESC, ts_ms ASC
  LIMIT ?5";

fn query_top_depth_examples(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<(Vec<DepthExample>, bool)> {
    // One row past the cap tells us whether more qualifying rows exist,
    // without a second COUNT query.
    let limit = (MAX_EVIDENCE_EXAMPLES + 1) as i64;
    let mut statement = conn.prepare(TOP_DEPTH_EXAMPLES_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence,
        limit
    ])?;
    let mut examples = Vec::new();
    while let Some(row) = rows.next()? {
        let ts_ms: i64 = row.get(0)?;
        let depth: i64 = row.get(1)?;
        let model: Option<String> = row.get(2)?;
        let model =
            model.map(|model| cap_string("context.top_depth_examples.model", &model, diagnostics));
        examples.push(DepthExample {
            ts_ms,
            depth_tokens: as_u64(depth),
            model,
        });
    }
    let capped = examples.len() > MAX_EVIDENCE_EXAMPLES;
    if capped {
        examples.truncate(MAX_EVIDENCE_EXAMPLES);
        note_collection_cap(diagnostics, "context.top_depth_examples");
    }
    Ok((examples, capped))
}

/* --------------------------------------------------------------------
 * Per-model token totals.
 * ----------------------------------------------------------------- */

const BY_MODEL_SQL: &str = "SELECT model, input_tokens, output_tokens, cache_read_tokens,
        cache_write_tokens, ts_ms
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND role = 'assistant' AND model IS NOT NULL
  ORDER BY rowid";

fn query_by_model(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<(BTreeMap<String, ModelTokens>, bool)> {
    let mut statement = conn.prepare(BY_MODEL_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence
    ])?;
    let mut models: BTreeMap<String, ModelTokens> = BTreeMap::new();
    let mut capped = false;
    while let Some(row) = rows.next()? {
        let model: String = row.get(0)?;
        let input: i64 = row.get(1)?;
        let output: i64 = row.get(2)?;
        let cache_read: i64 = row.get(3)?;
        let cache_creation: i64 = row.get(4)?;
        let ts_ms: Option<i64> = row.get(5)?;
        let capped_name = cap_string("models.by_model", &model, diagnostics);
        if capped_name.len() != model.len() {
            capped = true;
        }
        if let Some(tokens) = models.get_mut(&capped_name) {
            add_model_tokens(tokens, input, output, cache_read, cache_creation, ts_ms);
        } else if models.len() == MAX_MODELS {
            capped = true;
            note_collection_cap(diagnostics, "models.by_model");
        } else {
            let mut tokens = ModelTokens::default();
            add_model_tokens(
                &mut tokens,
                input,
                output,
                cache_read,
                cache_creation,
                ts_ms,
            );
            models.insert(capped_name, tokens);
        }
    }
    Ok((models, capped))
}

/// Mirrors `evidence_sink::add_model_tokens`'s exact semantics, including
/// one quirk: while a model's `first_ts_ms` still holds its default of
/// `0`, the next timestamped turn overwrites it outright instead of
/// taking the minimum.
fn add_model_tokens(
    tokens: &mut ModelTokens,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    ts_ms: Option<i64>,
) {
    tokens.input = tokens.input.saturating_add(as_u64(input));
    tokens.output = tokens.output.saturating_add(as_u64(output));
    tokens.cache_read = tokens.cache_read.saturating_add(as_u64(cache_read));
    tokens.cache_creation = tokens.cache_creation.saturating_add(as_u64(cache_creation));
    tokens.turns = tokens.turns.saturating_add(1);
    if let Some(ts_ms) = ts_ms {
        if tokens.turns == 1 || tokens.first_ts_ms == 0 {
            tokens.first_ts_ms = ts_ms;
        } else {
            tokens.first_ts_ms = tokens.first_ts_ms.min(ts_ms);
        }
        tokens.last_ts_ms = tokens.last_ts_ms.max(ts_ms);
    }
}

/* --------------------------------------------------------------------
 * Dominant main-loop model.
 * ----------------------------------------------------------------- */

/// Groups `scope = 'main'` assistant turns by model and picks the one
/// with the most summed `output_tokens` (Cadence
/// `dominant_subagent_tier`, `crates/analysis/src/efficiency_findings.rs`:
/// the dominant tier is the one with the most output tokens, not the
/// most turns). A tie breaks on turn count (`COUNT(*)` per model,
/// descending), then the earliest `last_ts_ms` (`MAX(ts_ms)` per model,
/// ascending — the model whose activity ended soonest), then the
/// earliest `turn_index` (`MIN(turn_index)` per model, ascending).
/// Cadence breaks ties by a fixed tier-priority order instead; that
/// order has no antiburn equivalent (this query has no premium/tier
/// concept), so this tie-break is only a stable, deterministic
/// substitute. `LIMIT 1` keeps this a single bounded row.
const DOMINANT_MAIN_MODEL_SQL: &str = "SELECT model
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND scope = 'main' AND role = 'assistant' AND model IS NOT NULL
  GROUP BY model
  ORDER BY SUM(output_tokens) DESC, COUNT(*) DESC, MAX(ts_ms) ASC, MIN(turn_index) ASC
  LIMIT 1";

fn query_dominant_main_model(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<Option<String>> {
    let mut statement = conn.prepare(DOMINANT_MAIN_MODEL_SQL)?;
    let model: Option<String> = statement
        .query_row(
            params![key.environment_key, key.agent, key.session_id, claim_fence],
            |row| row.get(0),
        )
        .optional()?;
    Ok(model.map(|model| cap_string("models.dominant_main_model", &model, diagnostics)))
}

/* --------------------------------------------------------------------
 * Effort tiers and fast modes.
 * ----------------------------------------------------------------- */

const EFFORT_TIERS_SQL: &str = "SELECT effort, scope
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND effort IS NOT NULL
  ORDER BY rowid";

const FAST_MODES_SQL: &str = "SELECT speed, scope
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND speed IS NOT NULL
  ORDER BY rowid";

fn query_tier_map(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    sql: &str,
    field: &'static str,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<(BTreeMap<String, TurnCounts>, bool)> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence
    ])?;
    let mut map: BTreeMap<String, TurnCounts> = BTreeMap::new();
    let mut capped = false;
    while let Some(row) = rows.next()? {
        let value: String = row.get(0)?;
        let scope: String = row.get(1)?;
        let delegated = scope == "delegated";
        let capped_value = cap_string(field, &value, diagnostics);
        if capped_value.len() != value.len() {
            capped = true;
        }
        if let Some(counts) = map.get_mut(&capped_value) {
            increment_turn_count(counts, delegated);
        } else if map.len() == MAX_TIER_LABELS {
            capped = true;
            note_collection_cap(diagnostics, field);
        } else {
            let mut counts = TurnCounts::default();
            increment_turn_count(&mut counts, delegated);
            map.insert(capped_value, counts);
        }
    }
    Ok((map, capped))
}

fn increment_turn_count(counts: &mut TurnCounts, delegated: bool) {
    if delegated {
        counts.delegated = counts.delegated.saturating_add(1);
    } else {
        counts.main_loop = counts.main_loop.saturating_add(1);
    }
}

/* --------------------------------------------------------------------
 * Effort/speed signal coverage.
 * ----------------------------------------------------------------- */

const SIGNAL_COVERAGE_SQL: &str = "SELECT
    COALESCE(SUM(role = 'assistant' AND model IS NOT NULL), 0),
    COALESCE(SUM(role = 'assistant' AND model IS NOT NULL AND effort IS NOT NULL), 0),
    COALESCE(SUM(role = 'assistant' AND model IS NOT NULL AND speed IS NOT NULL), 0)
  FROM turn
 WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4";

fn query_signal_coverage(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<(SignalCoverage, SignalCoverage)> {
    conn.query_row(
        SIGNAL_COVERAGE_SQL,
        params![key.environment_key, key.agent, key.session_id, claim_fence],
        |row| {
            let eligible: i64 = row.get(0)?;
            let effort_present: i64 = row.get(1)?;
            let speed_present: i64 = row.get(2)?;
            let eligible_turns = as_u64(eligible);
            Ok((
                SignalCoverage {
                    eligible_turns,
                    present_turns: as_u64(effort_present),
                },
                SignalCoverage {
                    eligible_turns,
                    present_turns: as_u64(speed_present),
                },
            ))
        },
    )
}

/* --------------------------------------------------------------------
 * Delegated models.
 * ----------------------------------------------------------------- */

const DELEGATED_MODELS_SQL: &str = "SELECT model
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND scope = 'delegated' AND role = 'assistant' AND model IS NOT NULL
  ORDER BY rowid";

fn query_delegated_models(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<(BTreeSet<String>, bool)> {
    let mut statement = conn.prepare(DELEGATED_MODELS_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence
    ])?;
    let mut models = BTreeSet::new();
    let mut capped = false;
    while let Some(row) = rows.next()? {
        let model: String = row.get(0)?;
        let capped_model = cap_string("subagents.delegated_models", &model, diagnostics);
        if capped_model.len() != model.len() {
            capped = true;
        }
        if models.contains(&capped_model) {
            continue;
        }
        if models.len() == MAX_SUBAGENT_MODELS {
            capped = true;
            note_collection_cap(diagnostics, "subagents.delegated_models");
        } else {
            models.insert(capped_model);
        }
    }
    Ok((models, capped))
}

/* --------------------------------------------------------------------
 * Model transitions and idle gaps: one ordered scan of the main-loop
 * rows, per thread.
 * ----------------------------------------------------------------- */

const MAIN_THREAD_SCAN_SQL: &str = "SELECT thread_id, ts_ms, model
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND scope = 'main'
  ORDER BY thread_id, turn_index";

/// Mirrors `evidence_sink::observe_cache_and_compaction`'s model-transition
/// and idle-gap tracking exactly, with one difference: this scan resets
/// the active model and the previous timestamp at each `thread_id`
/// boundary, so a transition or a gap never crosses two threads.
fn query_transitions_and_idle_gaps(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<(Vec<ModelTransition>, bool, i64, i64)> {
    let mut statement = conn.prepare(MAIN_THREAD_SCAN_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence
    ])?;
    let mut transitions = Vec::new();
    let mut capped = false;
    let mut longest_idle_gap_ms: i64 = 0;
    let mut idle_gap_ms_total: i64 = 0;
    let mut current_thread: Option<String> = None;
    let mut active_model: Option<String> = None;
    let mut previous_ts: Option<i64> = None;
    while let Some(row) = rows.next()? {
        let thread_id: String = row.get(0)?;
        let ts_ms: Option<i64> = row.get(1)?;
        let model: Option<String> = row.get(2)?;
        if current_thread.as_deref() != Some(thread_id.as_str()) {
            current_thread = Some(thread_id);
            active_model = None;
            previous_ts = None;
        }
        if let Some(model) = model.as_deref() {
            if let Some(previous) = active_model.as_deref()
                && previous != model
                && let Some(ts_ms) = ts_ms
            {
                let from_model =
                    cap_string("cache.model_transitions.from_model", previous, diagnostics);
                let to_model = cap_string("cache.model_transitions.to_model", model, diagnostics);
                if from_model.len() != previous.len() || to_model.len() != model.len() {
                    capped = true;
                }
                if transitions.len() == MAX_MODEL_TRANSITIONS {
                    capped = true;
                    note_collection_cap(diagnostics, "cache.model_transitions");
                } else {
                    transitions.push(ModelTransition {
                        ts_ms,
                        from_model,
                        to_model,
                    });
                }
            }
            active_model = Some(model.to_owned());
        }
        if let Some(ts_ms) = ts_ms {
            if let Some(previous) = previous_ts {
                let gap = ts_ms.saturating_sub(previous).max(0);
                longest_idle_gap_ms = longest_idle_gap_ms.max(gap);
                idle_gap_ms_total = idle_gap_ms_total.saturating_add(gap);
            }
            previous_ts = Some(ts_ms);
        }
    }
    Ok((transitions, capped, longest_idle_gap_ms, idle_gap_ms_total))
}

/* --------------------------------------------------------------------
 * Compactions.
 * ----------------------------------------------------------------- */

const MANUAL_COMPACTIONS_SQL: &str = "SELECT COALESCE(
        SUM(is_compaction_boundary = 1 AND compaction_trigger = ?5), 0)
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND scope = 'main'";

fn query_manual_compactions(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<u64> {
    let count: i64 = conn.query_row(
        MANUAL_COMPACTIONS_SQL,
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            claim_fence,
            CompactionTrigger::Manual.as_str()
        ],
        |row| row.get(0),
    )?;
    Ok(as_u64(count))
}

const COMPACTION_BOUNDARIES_SQL: &str = "SELECT ts_ms, compaction_trigger,
        compaction_pre_tokens, compaction_post_tokens
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND scope = 'main' AND is_compaction_boundary = 1
  ORDER BY thread_id, turn_index
  LIMIT ?5";

fn query_compaction_boundaries(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    diagnostics: &mut ParseDiagnostics,
) -> rusqlite::Result<(Vec<CompactionBoundary>, bool)> {
    let limit = (MAX_COMPACTION_BOUNDARIES + 1) as i64;
    let mut statement = conn.prepare(COMPACTION_BOUNDARIES_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence,
        limit
    ])?;
    let mut boundaries = Vec::new();
    while let Some(row) = rows.next()? {
        let ts_ms: Option<i64> = row.get(0)?;
        let trigger: Option<String> = row.get(1)?;
        let pre_tokens: Option<i64> = row.get(2)?;
        let post_tokens: Option<i64> = row.get(3)?;
        boundaries.push(CompactionBoundary {
            ts_ms: ts_ms.unwrap_or(0),
            trigger: trigger.as_deref().and_then(CompactionTrigger::parse),
            pre_tokens: pre_tokens.map(as_u64),
            post_tokens: post_tokens.map(as_u64),
        });
    }
    let capped = boundaries.len() > MAX_COMPACTION_BOUNDARIES;
    if capped {
        boundaries.truncate(MAX_COMPACTION_BOUNDARIES);
        note_collection_cap(diagnostics, "compactions.boundaries");
    }
    Ok((boundaries, capped))
}

/* --------------------------------------------------------------------
 * Duplicate thread identities.
 * ----------------------------------------------------------------- */

const DUPLICATE_TURN_IDENTITIES_SQL: &str = "SELECT COUNT(*) FROM (
        SELECT uuid FROM turn
         WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
           AND uuid IS NOT NULL
         GROUP BY uuid
        HAVING COUNT(DISTINCT source_key) > 1
    )";

fn query_duplicate_turn_identities(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<u64> {
    let count: i64 = conn.query_row(
        DUPLICATE_TURN_IDENTITIES_SQL,
        params![key.environment_key, key.agent, key.session_id, claim_fence],
        |row| row.get(0),
    )?;
    Ok(as_u64(count))
}

/* --------------------------------------------------------------------
 * Repeated-context accounting: paid context beyond positive growth,
 * summed over adjacent `scope = 'main'` assistant turn pairs within one
 * thread. See `RepeatedContext` in `evidence.rs`.
 * ----------------------------------------------------------------- */

/// A sibling scan to [`query_transitions_and_idle_gaps`]'s
/// `MAIN_THREAD_SCAN_SQL`: it filters to assistant rows only, because a
/// user or tool row carries no usage to pair.
const REPEATED_CONTEXT_SCAN_SQL: &str = "SELECT thread_id, ts_ms, input_tokens,
        cache_read_tokens, cache_write_tokens, is_compaction_boundary
   FROM turn
  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4
    AND scope = 'main' AND role = 'assistant'
  ORDER BY thread_id, turn_index";

struct RepeatedContextTotals {
    cache_write_tokens: u64,
    uncached_input_tokens: u64,
    pairs_considered: u64,
    pairs_skipped: u64,
}

/// One pass over `scope = 'main'` assistant rows, ordered by thread then
/// turn. Resets the previous row at each `thread_id` boundary, so a pair
/// never crosses two threads — the same rule
/// [`query_transitions_and_idle_gaps`] follows. Computes both candidate
/// sums (cache-write and uncached-input); the sink picks the one the
/// source's capabilities support.
///
/// `is_compaction_boundary` is read but not used here. A compaction
/// boundary row is a pair member like any other row: the detector
/// explains a finding's cause from `model_transitions`,
/// `longest_idle_gap_ms`, and `user_controlled_churn`, not from this sum.
fn query_repeated_context(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<RepeatedContextTotals> {
    let mut statement = conn.prepare(REPEATED_CONTEXT_SCAN_SQL)?;
    let mut rows = statement.query(params![
        key.environment_key,
        key.agent,
        key.session_id,
        claim_fence
    ])?;
    let mut cache_write_tokens: u64 = 0;
    let mut uncached_input_tokens: u64 = 0;
    let mut pairs_considered: u64 = 0;
    let mut pairs_skipped: u64 = 0;
    let mut current_thread: Option<String> = None;
    let mut previous: Option<(Option<i64>, i64)> = None;
    while let Some(row) = rows.next()? {
        let thread_id: String = row.get(0)?;
        let ts_ms: Option<i64> = row.get(1)?;
        let input_tokens: i64 = row.get(2)?;
        let cache_read_tokens: i64 = row.get(3)?;
        let cache_write: i64 = row.get(4)?;
        let _is_compaction_boundary: bool = row.get(5)?;
        let depth = input_tokens + cache_read_tokens + cache_write;
        if current_thread.as_deref() != Some(thread_id.as_str()) {
            current_thread = Some(thread_id);
            previous = None;
        }
        if let Some((previous_ts, previous_depth)) = previous {
            let in_order = matches!((previous_ts, ts_ms), (Some(previous_ts), Some(ts_ms)) if ts_ms >= previous_ts);
            if in_order {
                pairs_considered += 1;
                let growth = as_u64((depth - previous_depth).max(0));
                let paid_cache_write = as_u64(cache_write);
                let paid_uncached_input = as_u64(input_tokens);
                cache_write_tokens =
                    cache_write_tokens.saturating_add(paid_cache_write.saturating_sub(growth));
                uncached_input_tokens = uncached_input_tokens
                    .saturating_add(paid_uncached_input.saturating_sub(growth));
            } else {
                pairs_skipped += 1;
            }
        }
        previous = Some((ts_ms, depth));
    }
    Ok(RepeatedContextTotals {
        cache_write_tokens,
        uncached_input_tokens,
        pairs_considered,
        pairs_skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::EVIDENCE_STRING_CAP;
    use crate::analysis::rows::{TURN_MIGRATIONS, TurnRow, TurnScope, insert_turn_rows};

    const KEY: TurnSessionKey<'static> = TurnSessionKey {
        environment_key: "native",
        agent: "claude",
        session_id: "s1",
    };

    fn test_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory connection");
        conn.execute_batch(
            "CREATE TABLE session (
                environment_key TEXT NOT NULL,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                PRIMARY KEY (environment_key, agent, session_id)
            ) STRICT;",
        )
        .expect("create session table");
        for migration in TURN_MIGRATIONS {
            conn.execute_batch(migration)
                .expect("apply turn schema migration");
        }
        conn.execute(
            "INSERT INTO session (environment_key, agent, session_id) VALUES (?1, ?2, ?3)",
            params![KEY.environment_key, KEY.agent, KEY.session_id],
        )
        .expect("insert session");
        conn
    }

    fn base_row(thread_id: &str, turn_index: u64) -> TurnRow {
        TurnRow {
            source_key: thread_id.to_owned(),
            thread_id: thread_id.to_owned(),
            turn_index,
            scope: TurnScope::Main,
            child_id: None,
            role: "assistant",
            ts_ms: Some(1_000 + turn_index as i64),
            model: Some("model-a".to_owned()),
            effort: None,
            speed: None,
            input_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 5,
            is_compaction_boundary: false,
            message_id: None,
            uuid: None,
            parent_uuid: None,
            compaction_trigger: None,
            compaction_pre_tokens: None,
            compaction_post_tokens: None,
            content: Vec::new(),
        }
    }

    fn insert(conn: &Connection, rows: &[TurnRow]) {
        insert_turn_rows(conn, &KEY, 1, rows).expect("insert rows");
    }

    #[test]
    fn an_empty_session_reads_as_all_zero() {
        let conn = test_connection();
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.eligibility.turns, 0);
        assert_eq!(facts.max_request_context_tokens, 0);
        assert_eq!(
            facts.time_range,
            SessionTimeRange {
                first_ts_ms: 0,
                last_ts_ms: 0,
                timestamped_turns: 0,
            }
        );
        assert!(facts.by_model.is_empty());
        assert!(facts.model_transitions.is_empty());
        assert!(facts.compaction_boundaries.is_empty());
        assert_eq!(facts.duplicate_turn_identities, 0);
        assert!(!facts.thread_identity_missing);
        assert_eq!(facts.repeated_context_cache_write_tokens, 0);
        assert_eq!(facts.repeated_context_uncached_input_tokens, 0);
        assert_eq!(facts.repeated_context_pairs_considered, 0);
        assert_eq!(facts.repeated_context_pairs_skipped, 0);
    }

    #[test]
    fn fence_filtering_ignores_rows_under_another_fence() {
        let conn = test_connection();
        insert_turn_rows(&conn, &KEY, 2, &[base_row("s1", 0)]).expect("insert other fence");
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.eligibility.turns, 0);
    }

    #[test]
    fn models_by_model_caps_at_max_models_and_flags_the_overflow() {
        let conn = test_connection();
        let rows: Vec<TurnRow> = (0..(MAX_MODELS * 2))
            .map(|index| {
                let mut row = base_row("s1", index as u64);
                row.model = Some(format!("model-{index}"));
                row
            })
            .collect();
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.by_model.len(), MAX_MODELS);
        assert!(facts.models_capped);
        assert!(
            facts
                .diagnostics
                .capped_collections
                .contains("models.by_model")
        );
    }

    #[test]
    fn tier_labels_cap_at_max_tier_labels_and_flag_the_overflow() {
        let conn = test_connection();
        let rows: Vec<TurnRow> = (0..(MAX_TIER_LABELS * 2))
            .map(|index| {
                let mut row = base_row("s1", index as u64);
                row.effort = Some(format!("tier-{index}"));
                row
            })
            .collect();
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.effort_tiers.len(), MAX_TIER_LABELS);
        assert!(facts.tiers_capped);
    }

    #[test]
    fn top_depth_examples_cap_at_max_evidence_examples_and_keep_the_deepest() {
        let conn = test_connection();
        let rows: Vec<TurnRow> = (0..(MAX_EVIDENCE_EXAMPLES * 2))
            .map(|index| {
                let mut row = base_row("s1", index as u64);
                row.input_tokens = index as u64 + 1;
                row
            })
            .collect();
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.top_depth_examples.len(), MAX_EVIDENCE_EXAMPLES);
        assert!(facts.depth_examples_capped);
        // The deepest example is kept; depth is `input_tokens` here since
        // every other component of depth is zero.
        assert_eq!(
            facts.top_depth_examples[0].depth_tokens,
            (MAX_EVIDENCE_EXAMPLES * 2) as u64
        );
    }

    #[test]
    fn model_transitions_cap_at_max_model_transitions_and_flag_the_overflow() {
        let conn = test_connection();
        let rows: Vec<TurnRow> = (0..((MAX_MODEL_TRANSITIONS + 1) * 2))
            .map(|index| {
                let mut row = base_row("s1", index as u64);
                row.model = Some(format!("model-{index}"));
                row
            })
            .collect();
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.model_transitions.len(), MAX_MODEL_TRANSITIONS);
        assert!(facts.transitions_capped);
    }

    #[test]
    fn a_long_transition_model_name_flags_the_overflow_without_capping_the_count() {
        let conn = test_connection();
        let mut first = base_row("s1", 0);
        first.model = Some("model-a".to_owned());
        let mut second = base_row("s1", 1);
        second.model = Some("x".repeat(EVIDENCE_STRING_CAP * 2));
        insert(&conn, &[first, second]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.model_transitions.len(), 1);
        assert_eq!(
            facts.model_transitions[0].to_model.len(),
            EVIDENCE_STRING_CAP
        );
        assert!(facts.transitions_capped);
    }

    #[test]
    fn models_by_model_key_truncation_flags_the_overflow_without_capping_the_count() {
        let conn = test_connection();
        let mut row = base_row("s1", 0);
        row.model = Some("x".repeat(EVIDENCE_STRING_CAP * 2));
        insert(&conn, &[row]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.by_model.len(), 1);
        assert_eq!(
            facts.by_model.keys().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
        assert!(facts.models_capped);
    }

    fn tier_label_key_truncation(effort: bool) -> TurnFacts {
        let conn = test_connection();
        let mut row = base_row("s1", 0);
        if effort {
            row.effort = Some("x".repeat(EVIDENCE_STRING_CAP * 2));
        } else {
            row.speed = Some("x".repeat(EVIDENCE_STRING_CAP * 2));
        }
        insert(&conn, &[row]);
        query_turn_facts(&conn, &KEY, 1).expect("query facts")
    }

    #[test]
    fn effort_tier_key_truncation_flags_the_overflow() {
        let facts = tier_label_key_truncation(true);
        assert_eq!(facts.effort_tiers.len(), 1);
        assert_eq!(
            facts.effort_tiers.keys().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
        assert!(facts.tiers_capped);
    }

    #[test]
    fn fast_mode_key_truncation_flags_the_overflow() {
        let facts = tier_label_key_truncation(false);
        assert_eq!(facts.fast_modes.len(), 1);
        assert_eq!(
            facts.fast_modes.keys().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
        assert!(facts.tiers_capped);
    }

    #[test]
    fn delegated_models_cap_at_max_subagent_models_and_flag_the_overflow() {
        let conn = test_connection();
        let rows: Vec<TurnRow> = (0..(MAX_SUBAGENT_MODELS * 2))
            .map(|index| {
                let mut row = base_row("s1", index as u64);
                row.scope = TurnScope::Delegated;
                row.child_id = Some("s1".to_owned());
                row.model = Some(format!("model-{index}"));
                row
            })
            .collect();
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.delegated_models.len(), MAX_SUBAGENT_MODELS);
        assert!(facts.delegated_models_capped);
    }

    #[test]
    fn a_long_delegated_model_name_flags_the_overflow_without_capping_the_count() {
        let conn = test_connection();
        let mut row = base_row("s1", 0);
        row.scope = TurnScope::Delegated;
        row.child_id = Some("s1".to_owned());
        row.model = Some("x".repeat(EVIDENCE_STRING_CAP * 2));
        insert(&conn, &[row]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.delegated_models.len(), 1);
        assert_eq!(
            facts.delegated_models.iter().next().unwrap().len(),
            EVIDENCE_STRING_CAP
        );
        assert!(facts.delegated_models_capped);
    }

    #[test]
    fn signal_coverage_counts_only_model_attributed_assistant_rows() {
        // A row without a model (a synthetic `<synthetic>` assistant record,
        // or any non-assistant row) is never signal-eligible: it can never
        // carry an effort or speed value.
        let conn = test_connection();
        let mut with_signal = base_row("s1", 0);
        with_signal.effort = Some("high".to_owned());
        with_signal.speed = Some("standard".to_owned());
        let mut unattributed = base_row("s1", 1);
        unattributed.model = None;
        let tool_row = TurnRow {
            role: "tool",
            ..base_row("s1", 2)
        };
        insert(&conn, &[with_signal, unattributed, tool_row]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.effort_signal.eligible_turns, 1);
        assert_eq!(facts.effort_signal.present_turns, 1);
        assert_eq!(facts.speed_signal.eligible_turns, 1);
        assert_eq!(facts.speed_signal.present_turns, 1);
    }

    #[test]
    fn compaction_boundaries_cap_at_max_compaction_boundaries_and_flag_the_overflow() {
        let conn = test_connection();
        let rows: Vec<TurnRow> = (0..(MAX_COMPACTION_BOUNDARIES * 2))
            .map(|index| {
                let mut row = base_row("s1", index as u64);
                row.is_compaction_boundary = true;
                row
            })
            .collect();
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.compaction_boundaries.len(), MAX_COMPACTION_BOUNDARIES);
        assert!(facts.compactions_capped);
    }

    #[test]
    fn two_threads_with_different_models_produce_no_transition_between_them() {
        let conn = test_connection();
        let mut first = base_row("thread-a", 0);
        first.model = Some("model-a".to_owned());
        let mut second = base_row("thread-b", 0);
        second.model = Some("model-b".to_owned());
        insert(&conn, &[first, second]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert!(facts.model_transitions.is_empty());
    }

    #[test]
    fn idle_gaps_reset_at_a_thread_boundary() {
        let conn = test_connection();
        let mut first_a = base_row("thread-a", 0);
        first_a.ts_ms = Some(0);
        let mut second_a = base_row("thread-a", 1);
        second_a.ts_ms = Some(10_000);
        let mut first_b = base_row("thread-b", 0);
        first_b.ts_ms = Some(50_000);
        insert(&conn, &[first_a, second_a, first_b]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        // A gap between the two threads' first rows must not appear: the
        // scan resets `previous_ts` at the `thread-b` boundary.
        assert_eq!(facts.longest_idle_gap_ms, 10_000);
        assert_eq!(facts.idle_gap_ms_total, 10_000);
    }

    #[test]
    fn delegated_rows_are_excluded_from_transitions_but_summed_into_tokens() {
        let conn = test_connection();
        let mut delegated = base_row("s1", 0);
        delegated.scope = TurnScope::Delegated;
        delegated.child_id = Some("s1".to_owned());
        delegated.model = Some("model-b".to_owned());
        delegated.input_tokens = 7;
        let mut main = base_row("s1", 1);
        main.model = Some("model-a".to_owned());
        insert(&conn, &[delegated, main]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        // Only one main-loop model is ever observed, so there is no
        // transition to record even though the session used two models.
        assert!(facts.model_transitions.is_empty());
        assert_eq!(facts.fresh_input_tokens, 17);
        assert_eq!(facts.delegated_turns, 1);
    }

    #[test]
    fn a_duplicate_uuid_across_two_source_keys_is_counted() {
        let conn = test_connection();
        let mut first = base_row("s1", 0);
        first.uuid = Some("dup".to_owned());
        let mut second = base_row("s1", 1);
        second.source_key = "s2".to_owned();
        second.uuid = Some("dup".to_owned());
        let mut unique = base_row("s1", 2);
        unique.uuid = Some("solo".to_owned());
        insert(&conn, &[first, second, unique]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.duplicate_turn_identities, 1);
    }

    #[test]
    fn dominant_main_model_picks_the_most_output_tokens_not_the_most_turns() {
        let conn = test_connection();
        let mut rows = Vec::new();
        // model-a: three turns, five output tokens each (15 total).
        for index in 0..3 {
            let mut row = base_row("s1", index);
            row.model = Some("model-a".to_owned());
            row.output_tokens = 5;
            rows.push(row);
        }
        // model-b: one turn, 100 output tokens. Fewer turns, more output.
        let mut single = base_row("s1", 3);
        single.model = Some("model-b".to_owned());
        single.output_tokens = 100;
        rows.push(single);
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.dominant_main_model.as_deref(), Some("model-b"));
    }

    #[test]
    fn dominant_main_model_breaks_an_output_token_tie_by_turn_count() {
        let conn = test_connection();
        let mut rows = Vec::new();
        // model-a: two turns totalling 10 output tokens.
        for index in 0..2 {
            let mut row = base_row("s1", index);
            row.model = Some("model-a".to_owned());
            row.output_tokens = 5;
            rows.push(row);
        }
        // model-b: one turn with 10 output tokens. Same total, fewer turns.
        let mut single = base_row("s1", 2);
        single.model = Some("model-b".to_owned());
        single.output_tokens = 10;
        rows.push(single);
        insert(&conn, &rows);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.dominant_main_model.as_deref(), Some("model-a"));
    }

    #[test]
    fn three_growing_turns_repeat_no_context() {
        let conn = test_connection();
        let mut first = base_row("s1", 0);
        first.input_tokens = 100;
        first.ts_ms = Some(1_000);
        let mut second = base_row("s1", 1);
        second.input_tokens = 150;
        second.cache_write_tokens = 20;
        second.ts_ms = Some(1_010);
        let mut third = base_row("s1", 2);
        third.input_tokens = 200;
        third.cache_write_tokens = 10;
        third.ts_ms = Some(1_020);
        insert(&conn, &[first, second, third]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.repeated_context_cache_write_tokens, 0);
        assert_eq!(facts.repeated_context_pairs_considered, 2);
        assert_eq!(facts.repeated_context_pairs_skipped, 0);
    }

    #[test]
    fn a_full_resend_after_a_model_switch_pays_beyond_growth() {
        let conn = test_connection();
        let mut first = base_row("s1", 0);
        first.model = Some("model-a".to_owned());
        first.input_tokens = 1_000;
        first.ts_ms = Some(1_000);
        let mut second = base_row("s1", 1);
        second.model = Some("model-b".to_owned());
        second.input_tokens = 0;
        second.cache_write_tokens = 5_000;
        second.ts_ms = Some(2_000);
        insert(&conn, &[first, second]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        // depth grew 1000 -> 5000 (+4000); paid cache-write is 5000, so
        // 5000 - 4000 = 1000 tokens are repeated, not grown.
        assert_eq!(facts.repeated_context_cache_write_tokens, 1_000);
        assert_eq!(facts.repeated_context_pairs_considered, 1);
    }

    #[test]
    fn two_threads_never_form_a_cross_thread_pair() {
        let conn = test_connection();
        let mut thread_a_first = base_row("thread-a", 0);
        thread_a_first.input_tokens = 1_000;
        thread_a_first.ts_ms = Some(0);
        let mut thread_a_second = base_row("thread-a", 1);
        // Depth stays flat (1000 -> 1000): the previous turn's content
        // moves from `input` to `cache_read`, and 50 tokens are paid
        // again as `cache_write` with no matching growth.
        thread_a_second.input_tokens = 0;
        thread_a_second.cache_read_tokens = 950;
        thread_a_second.cache_write_tokens = 50;
        thread_a_second.ts_ms = Some(10);
        let mut thread_b_first = base_row("thread-b", 0);
        thread_b_first.input_tokens = 100_000;
        thread_b_first.ts_ms = Some(20);
        insert(&conn, &[thread_a_first, thread_a_second, thread_b_first]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        // A wrongly-crossed pair between thread-a's last row and
        // thread-b's huge first row would swamp this sum.
        assert_eq!(facts.repeated_context_cache_write_tokens, 50);
        assert_eq!(facts.repeated_context_pairs_considered, 1);
    }

    #[test]
    fn a_delegated_row_between_two_main_rows_is_ignored() {
        let conn = test_connection();
        let mut first_main = base_row("s1", 0);
        first_main.input_tokens = 1_000;
        first_main.ts_ms = Some(0);
        let mut delegated = base_row("s1", 1);
        delegated.scope = TurnScope::Delegated;
        delegated.child_id = Some("s1".to_owned());
        delegated.input_tokens = 900_000;
        delegated.ts_ms = Some(5);
        let mut second_main = base_row("s1", 2);
        // Depth stays flat, so the whole cache write (30) is repeated.
        second_main.input_tokens = 0;
        second_main.cache_read_tokens = 970;
        second_main.cache_write_tokens = 30;
        second_main.ts_ms = Some(10);
        insert(&conn, &[first_main, delegated, second_main]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        // The delegated row's huge input never enters the scan (scope =
        // 'main' only), so the pair forms directly between the two main
        // rows: growth is 0 and the whole cache write (30) is repeated.
        assert_eq!(facts.repeated_context_cache_write_tokens, 30);
        assert_eq!(facts.repeated_context_pairs_considered, 1);
    }

    #[test]
    fn a_null_ts_ms_pair_is_skipped_and_counted() {
        let conn = test_connection();
        let mut first = base_row("s1", 0);
        first.ts_ms = Some(0);
        let mut second = base_row("s1", 1);
        second.ts_ms = None;
        second.cache_write_tokens = 40;
        insert(&conn, &[first, second]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        assert_eq!(facts.repeated_context_pairs_skipped, 1);
        assert_eq!(facts.repeated_context_pairs_considered, 0);
        assert_eq!(facts.repeated_context_cache_write_tokens, 0);
    }

    #[test]
    fn uncached_input_accounting_reads_repeated_input_beyond_growth_on_codex_shaped_rows() {
        let conn = test_connection();
        let mut first = base_row("s1", 0);
        first.input_tokens = 1_000;
        first.cache_write_tokens = 0;
        first.ts_ms = Some(0);
        let mut second = base_row("s1", 1);
        // Codex never writes cache_write_tokens; a full uncached resend
        // after cache expiry shows up as input_tokens alone.
        second.input_tokens = 6_000;
        second.cache_write_tokens = 0;
        second.ts_ms = Some(1_000);
        insert(&conn, &[first, second]);
        let facts = query_turn_facts(&conn, &KEY, 1).expect("query facts");
        // depth grew 1000 -> 6000 (+5000); paid uncached input is 6000, so
        // 6000 - 5000 = 1000 tokens are repeated.
        assert_eq!(facts.repeated_context_uncached_input_tokens, 1_000);
        assert_eq!(facts.repeated_context_cache_write_tokens, 0);
    }
}
