//! Vendor-agnostic analysis of AI coding-agent session transcripts.
//!
//! The caller decides *which* sessions to analyze and hands them in as
//! [`SessionInput`]s; this module owns everything after that: per-vendor
//! normalization (the [`interface`] layer) and the vendor-neutral [`engine`]
//! that turns the normalized stream into an [`ActiveSessionsSummary`].
//!
//! A second, independent pass ([`initial_context`]) attributes each session's
//! *initial* context window (tokens loaded before the first response) by source
//! — skills, MCP servers, agent instruction files, system instructions, and an
//! unattributed remainder. It reads the raw transcript directly because the
//! normalized stream drops the per-source text it needs.
//!
//! Everything here is local and read-only: transcripts are parsed from bytes the
//! caller supplies (or from a path/SQLite file on this machine), and no result
//! ever leaves the process.
//!
//! ```no_run
//! use antiburn_local::analysis::{analyze_sources, SessionInput, RawSource};
//!
//! let inputs = vec![SessionInput {
//!     agent: "claude".into(),
//!     session_id: "abc".into(),
//!     source: RawSource::File("/path/to/abc.jsonl".into()),
//! }];
//! let summary = analyze_sources(inputs);
//! println!("{} live sessions", summary.session_count);
//! ```

mod efficiency;
mod engine;
mod evidence;
mod evidence_query;
mod evidence_sink;
mod framing;
mod initial_context;
mod interface;
mod merge;
mod metrics_sink;
mod model;
mod pricing;
pub(crate) mod records;
mod replay;
mod rows;
mod source_validity;
pub(crate) mod threads;
pub mod tool_catalog;
mod vendors;

pub use efficiency::{EfficiencyTotals, thread_efficiency};
pub use engine::{
    ActiveSessionsSummary, BUCKETS, Bucket, CONTEXT_WINDOW, SessionCost, SessionMetrics, SkillUse,
    aggregate_metrics, analyze_session,
};
pub use evidence::{
    CacheEvidence, ChurnCounts, CompactionBoundary, CompactionEvidence, ContextEvidence,
    ContextSourceEvidence, CoverageReason, DepthExample, EVIDENCE_STRING_CAP, EligibilityEvidence,
    EvidenceCoverage, EvidenceSource, EvidenceValue, FAST_SPEED_KEY, LoadedSource, ModelEvidence,
    ModelTokens, ModelTransition, OrderingObservation, ParseDiagnostics, QuotaConfidence,
    QuotaHitSeverity, QuotaIncident, QuotaLimitKind, RelationConfidence, RepeatedContext,
    RepeatedContextAccounting, SessionEvidence, SessionEvidenceIdentity, SessionProvenance,
    SessionQuotaEvidence, SessionTimeRange, SignalCoverage, SourceAcceptance, SourceCapabilities,
    SourceKind, SubagentChild, SubagentEvidence, SubagentExample, ToolClass, ToolEvidence, ToolUse,
    TurnCounts,
};
pub use evidence_query::{
    TurnFacts, query_model_breakdown, query_model_runs, query_turn_facts, query_turn_rows,
};
pub use evidence_sink::{CompositeSink, RETAINED_EVIDENCE_BYTES_BOUND, SessionEvidenceAccumulator};
pub use framing::{
    BoundedJsonlReader, FramedRecord, MAX_RECORD_BYTES, PartialReason, RecordSkip,
    SCAN_QUANTUM_BYTES,
};
pub use initial_context::{InitialContextBreakdown, InitialContextSourceCount, SourceOrigin};
pub use interface::{
    ContentKind, ContentPart, ContextSourceKind, EvidenceObservation, MAX_CONTENT_PART_BYTES,
    MAX_PROVIDER_HINTS, NormalizedRecord, ProviderHint, RawSource, RecordCoverage, RecordSink,
    RelationProvenance, SessionCollector, SessionInput, SessionSummary, SourceChangedReason,
    TurnContent, VendorAdapter, VisitOutcome,
};
pub use merge::merge_subagent_events;
pub use metrics_sink::{RETAINED_METRICS_BYTES_BOUND, SessionMetricsAccumulator, merge_metrics};
pub use model::{
    EventSource, ModelRun, NormalizedEvent, NormalizedSession, Role, ToolCall, ToolCategory, Usage,
};
pub use pricing::{install_runtime_pricing, price_breakdown, pricing_generation};
pub use replay::{MissingParentRows, metrics_by_source, metrics_from_rows};
pub use rows::{
    MemoryTurnRowStore, TURN_MIGRATIONS, TURN_ROW_BATCH_SIZE, TURN_SCHEMA_SQL, TURN_SCHEMA_V2_SQL,
    TURN_SCHEMA_V3_SQL, TurnRow, TurnRowError, TurnRowSink, TurnRowStore, TurnScope,
    TurnSessionKey, count_turn_content_rows, count_turn_rows, delete_turn_rows,
    delete_turn_rows_except_fence, delete_turn_rows_for_fence, insert_turn_rows,
    turn_row_from_event,
};
pub use source_validity::{
    AppendOnlyGuarantee, PinnedOpen, PinnedReader, PinnedSource, SourceClaim, append_only_guarantee,
};
pub use vendors::claude::ClaudeAdapter;
pub use vendors::pi::PiAdapter;
pub use vendors::{adapter_for, has_dedicated_adapter};

// +1 for native Antigravity token classes and paired transcript roles. Stored
// database sessions must re-ingest for model and cache checks.
// +1 for Codex collab-family recognition: `collab_agent_spawn_begin`,
// `collab_agent_spawn_end`, `collab_agent_interaction_begin`,
// `collab_agent_interaction_end`, `collab_waiting_begin`,
// `collab_waiting_end`, `collab_close_begin`, `collab_close_end`,
// `collab_resume_begin`, and `collab_resume_end` are now recognized as
// eventless (`vendors::codex::is_recognized_eventless`), so a stored Codex
// collab session must re-ingest to clear its degraded `Partial` coverage.
pub const PARSER_REVISION: i64 = 19;
// +1 for turn row chart signals: `has_thinking`, `last_tool`, and
// `subagent_launches` are now ingest-derived row columns
// (`rows::turn_row_from_event`), so every session must reparse to
// populate them.
// +1 for Codex cache-write tokens: `codex_usage` now splits
// `cache_write_input_tokens` into `cache_creation_tokens` instead of
// folding it into `input_tokens`, so a stored Codex session must re-ingest
// to pick up the new split (`vendors::codex::codex_usage`).
// +1 for the fork sub-agent replay fix: the Claude adapter skips a fork
// transcript's replayed parent records at the ingest boundary, so a stored
// fork session must re-ingest to drop its duplicate turn rows and usage.
// +1 for the Codex `token_count` fix: a heartbeat with zero-component usage
// beside a nonzero derived `total_tokens` is now inert, and
// `cache_write_input_tokens` is now a known usage key
// (`vendors::codex::is_usage_free_token_count`).
// +1 for parts A-E of the Cadence model-tier-policy parity fixups: dropped
// `ultrathink`, canonical model-key namespacing, the replacement registry
// and premium-policy rewrites, and dominant-main-model-by-output-tokens
// (`insights::detectors`), on top of the pre-existing +1 for per-thread
// repeated-context accounting (`RepeatedContext`, below).
// +1 more for part F: Cache Churn now scores `RepeatedContext` by overpay
// multiple instead of an absolute token threshold (`insights::detectors::
// cache_churn`), so an old assessed outcome no longer matches this rule.
// +1 for the partial signal coverage fix: Overuse of Fast Mode and Model
// Overthinking now assess only the turns that report a speed or effort
// value, instead of demanding every eligible turn carry one. A session
// with subagent work is no longer always `SignalMissing`.
// +1 for `linear_record_order`: `previous_turn` now also attests complete
// from line order alone, for a source with no per-record id
// (`evidence_sink::SessionEvidenceAccumulator::evidence`), so a Codex
// session already assessed as `CapabilityMissing` may now assess clean.
pub const ANALYZER_REVISION: i64 = 16;
// +1 for seam R2: the worker path now derives `inclusive_model_breakdown`
// and `model_runs` from published turn rows instead of the accumulator
// (`query_model_breakdown`, `query_model_runs`), so every session in the
// durable evidence queue must requeue into the new shape
// (`reconcile_evidence_revisions`).
// +1 for per-bucket rewrite tokens. The Context chart uses them to show
// derived rewrite markers. Stored analyses must rerun to populate this field.
// +1 for persisted provider hints. Stored analyses must rerun to populate
// `session_analysis.provider_hints_json` from the bounded session summary.
pub const METRICS_SCHEMA_REVISION: i64 = 4;
// +1 for `RepeatedContext` (`evidence::CacheEvidence::repeated_context`).
// +1 more for `RepeatedContext::paid_tokens` (part F).
// +1 more for `SourceCapabilities::linear_record_order`.
pub const EVIDENCE_SCHEMA_REVISION: i64 = 12;

/// Normalize and analyze a batch of live sessions into one averaged summary.
///
/// Sources that fail to read (missing file, unopenable DB) are skipped so one
/// bad session never sinks the whole view. Returns an empty summary when no
/// session yields any analyzable events.
pub fn analyze_sources(inputs: Vec<SessionInput>) -> ActiveSessionsSummary {
    analyze_sources_with(inputs, true)
}

/// Like [`analyze_sources`], but lets the caller skip the initial-context pass.
///
/// The aggregate (multi-session) view never renders `initialContext` — only the
/// single-session view does — so the polled aggregate command passes `false` to
/// avoid an extra raw read + JSON parse per live session on every 8s tick.
pub fn analyze_sources_with(
    inputs: Vec<SessionInput>,
    want_initial_context: bool,
) -> ActiveSessionsSummary {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    // Normalize each source in isolation. A panic in one vendor adapter (an
    // arithmetic overflow on a malformed transcript, an unexpected shape) must
    // drop only that session — not fail the whole command for every live session.
    let sessions: Vec<NormalizedSession> = inputs
        .iter()
        .filter_map(|input| {
            match catch_unwind(AssertUnwindSafe(|| {
                adapter_for(&input.agent).normalize(input)
            })) {
                Ok(Ok(session)) => Some(session),
                // An unreadable source affects one session only.
                Ok(Err(error)) => {
                    ::tracing::debug!(
                        event = "analysis_source_unreadable",
                        agent = %input.agent,
                        session_id = %input.session_id,
                        error = %error
                    );
                    None
                }
                Err(_) => {
                    ::tracing::error!(
                        event = "analysis_adapter_panicked",
                        agent = %input.agent,
                        session_id = %input.session_id
                    );
                    None
                }
            }
        })
        .collect();
    let mut summary = engine::aggregate(&sessions);

    if !want_initial_context {
        return summary;
    }

    // The initial-context breakdown is a separate pass over the *raw* payload:
    // the normalized stream drops the per-source text (skill listings, MCP
    // deltas, base/developer prompts) it needs. Compute it per input and graft
    // it onto the matching session metrics. Sources we can't read as text here
    // (SQLite-backed agents) simply leave the field `None` ("unavailable").
    for input in &inputs {
        let payload = match vendors::read_source(&input.source) {
            Ok(payload) => payload,
            Err(error) => {
                ::tracing::trace!(
                    event = "initial_context_source_unreadable",
                    agent = %input.agent,
                    session_id = %input.session_id,
                    error = %error
                );
                continue;
            }
        };

        // Skill one-liners grafted onto each `SkillUse::description` by name. This
        // rides the same raw-payload read as the initial-context pass (so the
        // aggregate tick, which skips this whole block, pays nothing) but is
        // independent of it: a session with skills but no initial-context
        // breakdown still gets its descriptions. Isolated behind `catch_unwind`
        // like the breakdown — a malformed transcript must not sink the command.
        let descriptions = catch_unwind(AssertUnwindSafe(|| {
            initial_context::parse_skill_descriptions(&input.agent, &payload)
        }))
        .unwrap_or_default();

        let breakdown = match catch_unwind(AssertUnwindSafe(|| {
            initial_context::parse_initial_context(&input.agent, &payload)
        })) {
            Ok(breakdown) => breakdown,
            Err(_) => {
                ::tracing::error!(
                    event = "initial_context_parse_panicked",
                    agent = %input.agent,
                    session_id = %input.session_id
                );
                None
            }
        };

        if let Some(metrics) = summary
            .sessions
            .iter_mut()
            .find(|m| m.agent == input.agent && m.session_id == input.session_id)
        {
            if let Some(mut breakdown) = breakdown {
                initial_context::fill_use_counts(
                    &mut breakdown,
                    &metrics.skill_uses,
                    &metrics.mcp_tool_calls,
                    &metrics.tool_calls_by_name,
                );
                metrics.initial_context = Some(breakdown);
            }
            for skill_use in &mut metrics.skill_uses {
                if skill_use.description.is_none()
                    && let Some(description) = descriptions.get(&skill_use.name)
                {
                    skill_use.description = Some(description.clone());
                }
            }
        }
    }

    summary
}

/// Normalize a single source without aggregation (handy for tests/tools).
pub fn normalize_source(input: &SessionInput) -> anyhow::Result<NormalizedSession> {
    adapter_for(&input.agent).normalize(input)
}

#[cfg(test)]
mod tests;
