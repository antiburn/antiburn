// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::analysis::{PartialReason, RawSource, VisitOutcome};

pub const EVIDENCE_STRING_CAP: usize = 256;
pub const MAX_EVIDENCE_EXAMPLES: usize = 8;
pub const MAX_TOOL_NAMES: usize = 128;
pub const MAX_CONTEXT_SOURCES: usize = 64;
pub const MAX_UNRECOGNIZED_TYPES: usize = 16;
pub const MAX_DIAGNOSTIC_FIELDS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum EvidenceValue<T> {
    // TODO @agent: CH-009 will remove this.
    #[cfg(debug_assertions)]
    Unimplemented,
    Unsupported,
    Partial {
        observed: T,
        reason: CoverageReason,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapabilities {
    pub request_context_tokens: bool,
    pub cache_write_tokens: bool,
    pub timestamps_and_order: bool,
    pub tool_invocations: bool,
    pub skill_mcp_attribution: bool,
    pub tool_definitions: bool,
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
    if end < value.len() {
        insert_diagnostic_field(&mut diagnostics.truncated_strings, field);
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

/// Contains the source facts that an accumulator needs at construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceSource {
    pub agent: String,
    pub session_id: String,
    pub kind: SourceKind,
    pub capabilities: SourceCapabilities,
}

// TODO @agent: CH-009 will remove this.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnfinishedGroup;

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
    #[cfg(debug_assertions)]
    pub models: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub subagents: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub cache: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub compactions: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub quota_incidents: EvidenceValue<UnfinishedGroup>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
