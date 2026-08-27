use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analysis::interface::RelationProvenance;
use crate::analysis::{PartialReason, RawSource, VisitOutcome};

pub const EVIDENCE_STRING_CAP: usize = 256;
pub const MAX_EVIDENCE_EXAMPLES: usize = 8;
pub const MAX_TOOL_NAMES: usize = 128;
pub const MAX_CONTEXT_SOURCES: usize = 64;
pub const MAX_UNRECOGNIZED_TYPES: usize = 16;
pub const MAX_DIAGNOSTIC_FIELDS: usize = 16;
pub const MAX_MODELS: usize = 32;
pub const MAX_TIER_LABELS: usize = 16;
pub const MAX_SUBAGENT_CHILDREN: usize = 64;
pub const MAX_MODEL_TRANSITIONS: usize = 64;
pub const MAX_COMPACTION_BOUNDARIES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum EvidenceValue<T> {
    Unsupported,
    Partial { observed: T, reason: CoverageReason },
    Complete(T),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageReason {
    Oversized,
    MalformedRecord,
    IncompleteTail,
    Cancelled,
    ReadFailed,
    UnrecognizedRecordType,
    PinnedPrefix,
    CapExceeded,
    AttributionIncomplete,
}

impl From<PartialReason> for CoverageReason {
    fn from(reason: PartialReason) -> Self {
        match reason {
            PartialReason::Oversized => Self::Oversized,
            PartialReason::MalformedRecord => Self::MalformedRecord,
            PartialReason::IncompleteTail => Self::IncompleteTail,
            PartialReason::Cancelled => Self::Cancelled,
            PartialReason::ReadFailed => Self::ReadFailed,
            PartialReason::UnrecognizedRecordType => Self::UnrecognizedRecordType,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTimeRange {
    pub first_ts_ms: i64,
    pub last_ts_ms: i64,
    pub timestamped_turns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EligibilityEvidence {
    pub turns: u64,
    pub assistant_turns: u64,
    pub tool_turns: u64,
    pub depth_eligible_turns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEvidence {
    pub max_request_context_tokens: u64,
    pub top_depth_examples: Vec<DepthExample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepthExample {
    pub ts_ms: i64,
    pub depth_tokens: u64,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Mcp,
    Skill,
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUse {
    pub calls: u64,
    pub class: ToolClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvidence {
    pub by_name: BTreeMap<String, ToolUse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSource {
    pub description: Option<String>,
    pub invoked: bool,
    pub origin: EvidenceValue<()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSourceEvidence {
    pub skills: BTreeMap<String, LoadedSource>,
    pub mcp_servers: BTreeMap<String, LoadedSource>,
    pub tool_definitions: EvidenceValue<()>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub turns: u64,
    pub first_ts_ms: i64,
    pub last_ts_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCounts {
    pub main_loop: u64,
    pub delegated: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEvidence {
    pub by_model: BTreeMap<String, ModelTokens>,
    pub unattributed_turns: u64,
    pub effort_tiers: BTreeMap<String, TurnCounts>,
    pub fast_modes: BTreeMap<String, TurnCounts>,
    pub service_tiers: EvidenceValue<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationConfidence {
    Observed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentChild {
    pub ordinal: u32,
    pub parent_model: Option<String>,
    pub child_model: EvidenceValue<()>,
    pub confidence: RelationConfidence,
    pub provenance: RelationProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentExample {
    pub ts_ms: i64,
    pub parent_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentEvidence {
    pub spawn_count: u64,
    pub delegated_turns: u64,
    pub children: Vec<SubagentChild>,
    pub examples: Vec<SubagentExample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelTransition {
    pub ts_ms: i64,
    pub from_model: String,
    pub to_model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChurnCounts {
    pub manual_compactions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEvidence {
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    pub fresh_input_tokens: u64,
    pub model_transitions: Vec<ModelTransition>,
    pub longest_idle_gap_ms: i64,
    pub idle_gap_ms_total: i64,
    pub user_controlled_churn: ChurnCounts,
    pub previous_turn: EvidenceValue<()>,
    pub provider_eviction: EvidenceValue<()>,
}

/// Names the limit family that a transcript-observed quota incident reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaLimitKind {
    RollingWindow,
    Weekly,
    ModelSpecific,
    WeightedUsage,
    RateLimit,
}

/// Distinguishes a hard limit hit from an advance warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaHitSeverity {
    Warning,
    HardHit,
}

/// Records how the parser knows about an incident.
/// Only direct transcript observation is a valid source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaConfidence {
    Observed,
}

/// One transcript-observed quota or rate-limit incident.
/// The session identity carries the provider attribution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaIncident {
    pub ts_ms: i64,
    pub limit_kind: QuotaLimitKind,
    pub severity: QuotaHitSeverity,
    pub model: Option<String>,
    pub reset_ts_ms: Option<i64>,
    pub utilization_pct: Option<u8>,
    pub confidence: QuotaConfidence,
}

/// Transcript-attributable quota incidents for one session.
/// The parser bounds the collection and overflows to `Partial`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionQuotaEvidence {
    pub incidents: Vec<QuotaIncident>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionBoundary {
    pub ts_ms: i64,
    pub trigger: Option<crate::analysis::model::CompactionTrigger>,
    pub pre_tokens: Option<u64>,
    pub post_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionEvidence {
    pub boundaries: Vec<CompactionBoundary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapabilities {
    pub request_context_tokens: bool,
    pub cache_write_tokens: bool,
    pub timestamps_and_order: bool,
    pub tool_invocations: bool,
    pub skill_mcp_attribution: bool,
    pub tool_definitions: bool,
    pub model_identity: bool,
    pub token_classes: bool,
    pub reasoning_effort_tier: bool,
    pub fast_tier: bool,
    pub service_tier: bool,
    pub subagent_relationships: bool,
    pub subagent_models: bool,
    pub compaction_boundaries: bool,
    pub thread_identity: bool,
    pub quota_incidents: bool,
    pub harness_version: bool,
}

impl SourceCapabilities {
    pub fn claude() -> Self {
        Self {
            request_context_tokens: true,
            cache_write_tokens: true,
            timestamps_and_order: true,
            tool_invocations: true,
            skill_mcp_attribution: true,
            tool_definitions: false,
            model_identity: true,
            token_classes: true,
            reasoning_effort_tier: true,
            fast_tier: true,
            service_tier: false,
            subagent_relationships: true,
            subagent_models: false,
            compaction_boundaries: true,
            thread_identity: false,
            quota_incidents: false,
            harness_version: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCoverage {
    Complete,
    Partial(CoverageReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Jsonl,
    File,
    Sqlite,
}

impl From<&RawSource> for SourceKind {
    fn from(source: &RawSource) -> Self {
        match source {
            RawSource::Jsonl(_) => Self::Jsonl,
            RawSource::File(_) => Self::File,
            RawSource::Sqlite(_) => Self::Sqlite,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingObservation {
    Monotonic,
    OutOfOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceAcceptance {
    NotObserved,
    Unvalidated,
    AcceptedFull,
    AcceptedPrefix { boundary: u64 },
    SourceChanged,
}

impl From<VisitOutcome> for SourceAcceptance {
    fn from(outcome: VisitOutcome) -> Self {
        match outcome {
            VisitOutcome::Unvalidated => Self::Unvalidated,
            VisitOutcome::AcceptedFull => Self::AcceptedFull,
            VisitOutcome::AcceptedPrefix { boundary } => Self::AcceptedPrefix { boundary },
            VisitOutcome::SourceChanged(_) => Self::SourceChanged,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionProvenance {
    pub parser_revision: i64,
    pub analyzer_revision: i64,
    pub evidence_schema_revision: i64,
    pub source_kind: SourceKind,
    pub source_acceptance: SourceAcceptance,
    pub ordering: OrderingObservation,
    pub harness_version: EvidenceValue<()>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseDiagnostics {
    pub records_observed: u64,
    pub records_unusable: u64,
    pub unusable_reasons: BTreeMap<CoverageReason, u64>,
    pub unrecognized_types: BTreeSet<String>,
    pub truncated_strings: BTreeSet<String>,
    pub capped_collections: BTreeSet<String>,
}

impl ParseDiagnostics {
    pub(crate) fn new() -> Self {
        Self {
            records_observed: 0,
            records_unusable: 0,
            unusable_reasons: BTreeMap::new(),
            unrecognized_types: BTreeSet::new(),
            truncated_strings: BTreeSet::new(),
            capped_collections: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvidenceIdentity {
    pub agent: String,
    pub session_id: String,
}

impl SessionEvidenceIdentity {
    pub(crate) fn new(agent: &str, session_id: &str, diagnostics: &mut ParseDiagnostics) -> Self {
        Self {
            agent: cap_string("identity.agent", agent, diagnostics),
            session_id: cap_string("identity.session_id", session_id, diagnostics),
        }
    }
}

pub(crate) fn cap_string(
    field: &'static str,
    value: &str,
    diagnostics: &mut ParseDiagnostics,
) -> String {
    let mut end = value.len().min(EVIDENCE_STRING_CAP);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    if end < value.len() && insert_diagnostic_field(&mut diagnostics.truncated_strings, field) {
        record_diagnostic_set_cap(diagnostics, "diagnostics.truncated_strings");
    }
    value[..end].to_owned()
}

pub(crate) fn insert_diagnostic_field(set: &mut BTreeSet<String>, field: &'static str) -> bool {
    if set.contains(field) {
        return false;
    }
    if set.len() == MAX_DIAGNOSTIC_FIELDS {
        return true;
    }
    set.insert(field.to_owned());
    false
}

pub(crate) fn record_diagnostic_set_cap(diagnostics: &mut ParseDiagnostics, field: &'static str) {
    if diagnostics.capped_collections.contains(field) {
        return;
    }
    if diagnostics.capped_collections.len() == MAX_DIAGNOSTIC_FIELDS {
        diagnostics.capped_collections.pop_last();
        diagnostics
            .capped_collections
            .insert("diagnostics.capped_collections".to_owned());
        if field == "diagnostics.capped_collections" {
            return;
        }
        if diagnostics.capped_collections.len() == MAX_DIAGNOSTIC_FIELDS {
            diagnostics.capped_collections.pop_last();
        }
    }
    diagnostics.capped_collections.insert(field.to_owned());
}

/// Contains the source facts that an accumulator needs at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceSource {
    pub agent: String,
    pub session_id: String,
    pub kind: SourceKind,
    pub capabilities: SourceCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvidence {
    pub schema_revision: i64,
    pub identity: SessionEvidenceIdentity,
    pub context: EvidenceValue<ContextEvidence>,
    pub capabilities: SourceCapabilities,
    pub coverage: EvidenceCoverage,
    pub provenance: SessionProvenance,
    pub diagnostics: ParseDiagnostics,
    pub time_range: EvidenceValue<SessionTimeRange>,
    pub eligibility: EvidenceValue<EligibilityEvidence>,
    pub tools: EvidenceValue<ToolEvidence>,
    pub context_sources: EvidenceValue<ContextSourceEvidence>,
    pub models: EvidenceValue<ModelEvidence>,
    pub subagents: EvidenceValue<SubagentEvidence>,
    pub cache: EvidenceValue<CacheEvidence>,
    pub compactions: EvidenceValue<CompactionEvidence>,
    pub quota_incidents: EvidenceValue<SessionQuotaEvidence>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::analysis::SessionEvidenceAccumulator;

    fn empty_evidence(session_id: &str) -> SessionEvidence {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence()
    }

    fn expected_empty_evidence(
        session_id: String,
        coverage: serde_json::Value,
        truncated_strings: serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "schemaRevision": 2,
            "identity": {"agent": "claude", "sessionId": session_id},
            "context": {"state": "complete", "value": {"maxRequestContextTokens": 0, "topDepthExamples": []}},
            "capabilities": {
                "requestContextTokens": true,
                "cacheWriteTokens": true,
                "timestampsAndOrder": true,
                "toolInvocations": true,
                "skillMcpAttribution": true,
                "toolDefinitions": false,
                "modelIdentity": true,
                "tokenClasses": true,
                "reasoningEffortTier": true,
                "fastTier": true,
                "serviceTier": false,
                "subagentRelationships": true,
                "subagentModels": false,
                "compactionBoundaries": true,
                "threadIdentity": false,
                "quotaIncidents": false,
                "harnessVersion": false
            },
            "coverage": coverage,
            "provenance": {
                "parserRevision": 3,
                "analyzerRevision": 3,
                "evidenceSchemaRevision": 2,
                "sourceKind": "file",
                "sourceAcceptance": "not_observed",
                "ordering": "monotonic",
                "harnessVersion": {"state": "unsupported"}
            },
            "diagnostics": {
                "recordsObserved": 0,
                "recordsUnusable": 0,
                "unusableReasons": {},
                "unrecognizedTypes": [],
                "truncatedStrings": truncated_strings,
                "cappedCollections": []
            },
            "timeRange": {"state": "complete", "value": {"firstTsMs": 0, "lastTsMs": 0, "timestampedTurns": 0}},
            "eligibility": {"state": "complete", "value": {"turns": 0, "assistantTurns": 0, "toolTurns": 0, "depthEligibleTurns": 0}},
            "tools": {"state": "complete", "value": {"byName": {}}},
            "contextSources": {"state": "complete", "value": {"skills": {}, "mcpServers": {}, "toolDefinitions": {"state": "unsupported"}}},
            "models": {"state": "complete", "value": {"byModel": {}, "unattributedTurns": 0, "effortTiers": {}, "fastModes": {}, "serviceTiers": {"state": "unsupported"}}},
            "subagents": {"state": "complete", "value": {"spawnCount": 0, "delegatedTurns": 0, "children": [], "examples": []}},
            "cache": {"state": "complete", "value": {"cacheReadTokens": 0, "cacheCreationTokens": 0, "freshInputTokens": 0, "modelTransitions": [], "longestIdleGapMs": 0, "idleGapMsTotal": 0, "userControlledChurn": {"manualCompactions": 0}, "previousTurn": {"state": "unsupported"}, "providerEviction": {"state": "unsupported"}}},
            "compactions": {"state": "complete", "value": {"boundaries": []}},
            "quotaIncidents": {"state": "unsupported"}
        })
    }

    #[test]
    fn complete_session_evidence_serializes_to_the_exact_object() {
        assert_eq!(
            serde_json::to_value(empty_evidence("s1")).unwrap(),
            expected_empty_evidence("s1".to_owned(), json!("complete"), json!([]))
        );
    }

    #[test]
    fn partial_session_evidence_serializes_to_the_exact_object() {
        let long_id = "s".repeat(EVIDENCE_STRING_CAP + 1);
        assert_eq!(
            serde_json::to_value(empty_evidence(&long_id)).unwrap(),
            expected_empty_evidence(
                "s".repeat(EVIDENCE_STRING_CAP),
                json!({"partial": "cap_exceeded"}),
                json!(["identity.session_id"]),
            )
        );
    }

    #[test]
    fn evidence_value_serde_shape_is_adjacently_tagged() {
        let complete = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: 7,
            top_depth_examples: Vec::new(),
        });
        let partial = EvidenceValue::Partial {
            observed: ContextEvidence {
                max_request_context_tokens: 7,
                top_depth_examples: Vec::new(),
            },
            reason: CoverageReason::MalformedRecord,
        };
        let unsupported: EvidenceValue<ContextEvidence> = EvidenceValue::Unsupported;

        assert_eq!(
            serde_json::to_value(complete).unwrap(),
            json!({"state": "complete", "value": {"maxRequestContextTokens": 7, "topDepthExamples": []}})
        );
        assert_eq!(
            serde_json::to_value(partial).unwrap(),
            json!({"state": "partial", "value": {"observed": {"maxRequestContextTokens": 7, "topDepthExamples": []}, "reason": "malformed_record"}})
        );
        assert_eq!(
            serde_json::to_value(unsupported).unwrap(),
            json!({"state": "unsupported"})
        );
    }

    #[test]
    fn identity_strings_are_capped_and_diagnosed() {
        let prefix = "a".repeat(EVIDENCE_STRING_CAP - 1);
        let over_cap = format!("{prefix}ésuffix");
        let mut diagnostics = ParseDiagnostics::new();
        let identity = SessionEvidenceIdentity::new(&over_cap, &over_cap, &mut diagnostics);

        assert_eq!(identity.agent, prefix);
        assert_eq!(identity.session_id, prefix);
        assert_eq!(
            diagnostics.truncated_strings,
            BTreeSet::from([
                "identity.agent".to_owned(),
                "identity.session_id".to_owned()
            ])
        );
    }
}
