// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::analysis::{PartialReason, RawSource, VisitOutcome};

pub const EVIDENCE_STRING_CAP: usize = 256;

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
pub struct ContextEvidence {
    pub max_request_context_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCapabilities {
    pub request_context_tokens: bool,
    pub cache_write_tokens: bool,
}

impl SourceCapabilities {
    pub fn claude() -> Self {
        Self {
            request_context_tokens: true,
            cache_write_tokens: true,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvidenceIdentity {
    pub agent: String,
    pub session_id: String,
}

impl SessionEvidenceIdentity {
    pub fn new(agent: &str, session_id: &str) -> Self {
        Self {
            agent: capped_string(agent),
            session_id: capped_string(session_id),
        }
    }
}

fn capped_string(value: &str) -> String {
    let mut end = value.len().min(EVIDENCE_STRING_CAP);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
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
    // TODO @agent: CH-009 will remove this.
    #[cfg(debug_assertions)]
    pub time_range: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub eligibility: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub models: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub tools: EvidenceValue<UnfinishedGroup>,
    #[cfg(debug_assertions)]
    pub context_sources: EvidenceValue<UnfinishedGroup>,
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
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::json;

    use super::*;
    use crate::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION};

    #[test]
    fn evidence_value_serde_shape_is_adjacently_tagged() {
        let complete = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: 7,
        });
        let partial = EvidenceValue::Partial {
            observed: ContextEvidence {
                max_request_context_tokens: 7,
            },
            reason: CoverageReason::MalformedRecord,
        };
        let unsupported: EvidenceValue<ContextEvidence> = EvidenceValue::Unsupported;

        assert_eq!(
            serde_json::to_value(complete).unwrap(),
            json!({"state": "complete", "value": {"maxRequestContextTokens": 7}})
        );
        assert_eq!(
            serde_json::to_value(partial).unwrap(),
            json!({
                "state": "partial",
                "value": {
                    "observed": {"maxRequestContextTokens": 7},
                    "reason": "malformed_record"
                }
            })
        );
        assert_eq!(
            serde_json::to_value(unsupported).unwrap(),
            json!({"state": "unsupported"})
        );
    }

    #[test]
    fn evidence_value_round_trips_through_json() {
        let values = [
            EvidenceValue::Complete(ContextEvidence {
                max_request_context_tokens: 7,
            }),
            EvidenceValue::Partial {
                observed: ContextEvidence {
                    max_request_context_tokens: 7,
                },
                reason: CoverageReason::MalformedRecord,
            },
            EvidenceValue::Unsupported,
        ];

        for value in values {
            let encoded = serde_json::to_string(&value).unwrap();
            let decoded: EvidenceValue<ContextEvidence> = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, value);
        }
    }

    #[test]
    fn complete_zero_is_distinguishable_from_unsupported() {
        let complete = serde_json::to_value(EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: 0,
        }))
        .unwrap();
        let unsupported =
            serde_json::to_value(EvidenceValue::<ContextEvidence>::Unsupported).unwrap();

        assert_ne!(complete, unsupported);
        assert_ne!(complete, serde_json::Value::Null);
        assert_ne!(unsupported, serde_json::Value::Null);
    }

    #[test]
    #[cfg(debug_assertions)]
    fn unimplemented_serializes_as_its_own_state() {
        assert_eq!(
            serde_json::to_value(EvidenceValue::<UnfinishedGroup>::Unimplemented).unwrap(),
            json!({"state": "unimplemented"})
        );
    }

    #[test]
    fn complete_session_evidence_serializes_to_the_exact_object() {
        let evidence = session_evidence(
            EvidenceValue::Complete(ContextEvidence {
                max_request_context_tokens: 7,
            }),
            EvidenceCoverage::Complete,
            SourceAcceptance::AcceptedFull,
            BTreeMap::new(),
        );

        assert_eq!(
            serde_json::to_value(evidence).unwrap(),
            json!({
                "schemaRevision": 1,
                "identity": {"agent": "claude", "sessionId": "s1"},
                "context": {"state": "complete", "value": {"maxRequestContextTokens": 7}},
                "capabilities": {"requestContextTokens": true, "cacheWriteTokens": true},
                "coverage": "complete",
                "provenance": {
                    "parserRevision": 1,
                    "analyzerRevision": 1,
                    "evidenceSchemaRevision": 1,
                    "sourceKind": "file",
                    "sourceAcceptance": "accepted_full",
                    "ordering": "monotonic"
                },
                "diagnostics": {
                    "recordsObserved": 3,
                    "recordsUnusable": 0,
                    "unusableReasons": {}
                },
                "timeRange": {"state": "unimplemented"},
                "eligibility": {"state": "unimplemented"},
                "models": {"state": "unimplemented"},
                "tools": {"state": "unimplemented"},
                "contextSources": {"state": "unimplemented"},
                "subagents": {"state": "unimplemented"},
                "cache": {"state": "unimplemented"},
                "compactions": {"state": "unimplemented"},
                "quotaIncidents": {"state": "unimplemented"}
            })
        );
    }

    #[test]
    fn partial_session_evidence_serializes_to_the_exact_object() {
        let reasons = BTreeMap::from([(CoverageReason::MalformedRecord, 1)]);
        let evidence = session_evidence(
            EvidenceValue::Partial {
                observed: ContextEvidence {
                    max_request_context_tokens: 7,
                },
                reason: CoverageReason::MalformedRecord,
            },
            EvidenceCoverage::Partial(CoverageReason::MalformedRecord),
            SourceAcceptance::AcceptedPrefix { boundary: 4096 },
            reasons,
        );

        assert_eq!(
            serde_json::to_value(evidence).unwrap(),
            json!({
                "schemaRevision": 1,
                "identity": {"agent": "claude", "sessionId": "s1"},
                "context": {
                    "state": "partial",
                    "value": {
                        "observed": {"maxRequestContextTokens": 7},
                        "reason": "malformed_record"
                    }
                },
                "capabilities": {"requestContextTokens": true, "cacheWriteTokens": true},
                "coverage": {"partial": "malformed_record"},
                "provenance": {
                    "parserRevision": 1,
                    "analyzerRevision": 1,
                    "evidenceSchemaRevision": 1,
                    "sourceKind": "file",
                    "sourceAcceptance": {"accepted_prefix": {"boundary": 4096}},
                    "ordering": "monotonic"
                },
                "diagnostics": {
                    "recordsObserved": 3,
                    "recordsUnusable": 1,
                    "unusableReasons": {"malformed_record": 1}
                },
                "timeRange": {"state": "unimplemented"},
                "eligibility": {"state": "unimplemented"},
                "models": {"state": "unimplemented"},
                "tools": {"state": "unimplemented"},
                "contextSources": {"state": "unimplemented"},
                "subagents": {"state": "unimplemented"},
                "cache": {"state": "unimplemented"},
                "compactions": {"state": "unimplemented"},
                "quotaIncidents": {"state": "unimplemented"}
            })
        );
    }

    #[test]
    fn coverage_reason_maps_every_partial_reason() {
        let cases = [
            (
                PartialReason::Oversized,
                CoverageReason::Oversized,
                "oversized",
            ),
            (
                PartialReason::MalformedRecord,
                CoverageReason::MalformedRecord,
                "malformed_record",
            ),
            (
                PartialReason::IncompleteTail,
                CoverageReason::IncompleteTail,
                "incomplete_tail",
            ),
            (
                PartialReason::Cancelled,
                CoverageReason::Cancelled,
                "cancelled",
            ),
            (
                PartialReason::ReadFailed,
                CoverageReason::ReadFailed,
                "read_failed",
            ),
            (
                PartialReason::UnrecognizedRecordType,
                CoverageReason::UnrecognizedRecordType,
                "unrecognized_record_type",
            ),
        ];
        let mut mapped = BTreeSet::new();

        for (partial, expected, serialized) in cases {
            let actual = CoverageReason::from(partial);
            assert_eq!(actual, expected);
            assert_eq!(serde_json::to_value(actual).unwrap(), json!(serialized));
            mapped.insert(actual);
        }

        assert_eq!(mapped.len(), 6);
        assert!(!mapped.contains(&CoverageReason::PinnedPrefix));
    }

    #[test]
    fn identity_strings_are_capped() {
        let prefix = "a".repeat(EVIDENCE_STRING_CAP - 1);
        let over_cap = format!("{prefix}ésuffix");
        let identity = SessionEvidenceIdentity::new(&over_cap, &over_cap);

        assert!(identity.agent.len() <= EVIDENCE_STRING_CAP);
        assert!(identity.session_id.len() <= EVIDENCE_STRING_CAP);
        assert_eq!(identity.agent, prefix);
        assert_eq!(identity.session_id, prefix);
        assert!(identity.agent.is_char_boundary(identity.agent.len()));
        assert!(
            identity
                .session_id
                .is_char_boundary(identity.session_id.len())
        );
    }

    #[cfg(debug_assertions)]
    fn session_evidence(
        context: EvidenceValue<ContextEvidence>,
        coverage: EvidenceCoverage,
        source_acceptance: SourceAcceptance,
        unusable_reasons: BTreeMap<CoverageReason, u64>,
    ) -> SessionEvidence {
        SessionEvidence {
            schema_revision: EVIDENCE_SCHEMA_REVISION,
            identity: SessionEvidenceIdentity::new("claude", "s1"),
            context,
            capabilities: SourceCapabilities::claude(),
            coverage,
            provenance: SessionProvenance {
                parser_revision: PARSER_REVISION,
                analyzer_revision: ANALYZER_REVISION,
                evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                source_kind: SourceKind::File,
                source_acceptance,
                ordering: OrderingObservation::Monotonic,
            },
            diagnostics: ParseDiagnostics {
                records_observed: 3,
                records_unusable: u64::from(!unusable_reasons.is_empty()),
                unusable_reasons,
            },
            time_range: EvidenceValue::Unimplemented,
            eligibility: EvidenceValue::Unimplemented,
            models: EvidenceValue::Unimplemented,
            tools: EvidenceValue::Unimplemented,
            context_sources: EvidenceValue::Unimplemented,
            subagents: EvidenceValue::Unimplemented,
            cache: EvidenceValue::Unimplemented,
            compactions: EvidenceValue::Unimplemented,
            quota_incidents: EvidenceValue::Unimplemented,
        }
    }
}
