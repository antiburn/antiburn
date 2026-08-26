// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Quota-pressure section over transcript-attributable incidents.
//!
//! The section stays outside the nine-category detector contract
//! (FR-15). Its only input is the `quota_incidents` evidence group.
//! It calls no provider endpoint and reads no account-level limit
//! state. The section is not assessed exactly when the transcripts
//! carry no quota evidence — one condition, not a matrix. Presence
//! read from partial evidence is trustworthy, so partial evidence
//! with observed incidents still produces findings.

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{
    EvidenceValue, QuotaHitSeverity, QuotaLimitKind, SessionEvidenceIdentity, SessionQuotaEvidence,
};

use super::report::SessionExample;

/// Caps the reported affected-session examples.
pub const MAX_QUOTA_SESSION_EXAMPLES: usize = 3;
/// Caps the reported affected-model names.
pub const MAX_QUOTA_AFFECTED_MODELS: usize = 16;
/// Caps the reported example observation times.
pub const MAX_QUOTA_OBSERVED_TIMES: usize = 8;

/// One report-level quota-pressure result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaPressureSection {
    /// The transcripts carry no quota evidence.
    NotAssessed,
    Findings(QuotaPressureFindings),
}

/// Bounded summary of deduplicated transcript quota incidents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaPressureFindings {
    /// Deduplicated hit count per limit kind.
    pub hits_by_limit_kind: BTreeMap<QuotaLimitKind, u64>,
    /// Total deduplicated hits across all limit kinds.
    pub total_hits: u64,
    pub hard_hits: u64,
    pub warnings: u64,
    /// Count of sessions with at least one incident.
    pub affected_session_count: u64,
    /// Bounded session identities; no transcript content.
    pub affected_session_examples: Vec<SessionExample>,
    /// Bounded set of transcript-attributed model names.
    pub affected_models: BTreeSet<String>,
    /// True when more models were observed than the set retains.
    pub affected_models_truncated: bool,
    pub first_observed_ts_ms: i64,
    pub last_observed_ts_ms: i64,
    /// Bounded example observation times, ascending in each session.
    pub observed_times_ms: Vec<i64>,
}

/// Folds per-session quota evidence into the bounded section state.
#[derive(Debug, Default)]
pub(crate) struct QuotaPressureAccumulator {
    findings: Option<QuotaPressureFindings>,
}

impl QuotaPressureAccumulator {
    /// Observes one cohort session. Incidents are deduplicated within
    /// the session on time, limit kind, severity, and model, so a
    /// retried limit error logged twice counts once. The same incident
    /// observed by two sessions counts per session, because each
    /// session pays the interruption.
    pub(crate) fn observe_session(
        &mut self,
        identity: &SessionEvidenceIdentity,
        quota: &EvidenceValue<SessionQuotaEvidence>,
    ) {
        let incidents = match quota {
            EvidenceValue::Unsupported => return,
            EvidenceValue::Partial { observed, .. } => &observed.incidents,
            EvidenceValue::Complete(observed) => &observed.incidents,
        };
        // The key projects the incident identity — time, limit kind,
        // severity, and model. A re-logged limit error with a shifted
        // reset timestamp or a new utilization reading counts once.
        // The set is transient; CH-009 owns the per-session incident
        // cap that bounds the input collection.
        let deduplicated: BTreeSet<_> = incidents
            .iter()
            .map(|incident| {
                (
                    incident.ts_ms,
                    incident.limit_kind,
                    incident.severity,
                    incident.model.as_deref(),
                )
            })
            .collect();
        if deduplicated.is_empty() {
            return;
        }

        let findings = self.findings.get_or_insert_with(|| QuotaPressureFindings {
            hits_by_limit_kind: BTreeMap::new(),
            total_hits: 0,
            hard_hits: 0,
            warnings: 0,
            affected_session_count: 0,
            affected_session_examples: Vec::new(),
            affected_models: BTreeSet::new(),
            affected_models_truncated: false,
            first_observed_ts_ms: i64::MAX,
            last_observed_ts_ms: i64::MIN,
            observed_times_ms: Vec::new(),
        });
        findings.affected_session_count += 1;
        if findings.affected_session_examples.len() < MAX_QUOTA_SESSION_EXAMPLES {
            findings.affected_session_examples.push(SessionExample {
                agent: identity.agent.clone(),
                session_id: identity.session_id.clone(),
            });
        }
        for (ts_ms, limit_kind, severity, model) in deduplicated {
            findings.total_hits += 1;
            *findings.hits_by_limit_kind.entry(limit_kind).or_default() += 1;
            match severity {
                QuotaHitSeverity::HardHit => findings.hard_hits += 1,
                QuotaHitSeverity::Warning => findings.warnings += 1,
            }
            if let Some(model) = model {
                if findings.affected_models.contains(model) {
                    // Already retained.
                } else if findings.affected_models.len() < MAX_QUOTA_AFFECTED_MODELS {
                    findings.affected_models.insert(model.to_owned());
                } else {
                    findings.affected_models_truncated = true;
                }
            }
            findings.first_observed_ts_ms = findings.first_observed_ts_ms.min(ts_ms);
            findings.last_observed_ts_ms = findings.last_observed_ts_ms.max(ts_ms);
            if findings.observed_times_ms.len() < MAX_QUOTA_OBSERVED_TIMES {
                findings.observed_times_ms.push(ts_ms);
            }
        }
    }

    /// Finalizes the section. No observed incident anywhere means the
    /// section is not assessed — the one condition of FR-15.
    pub(crate) fn finish(self) -> QuotaPressureSection {
        match self.findings {
            None => QuotaPressureSection::NotAssessed,
            Some(findings) => QuotaPressureSection::Findings(findings),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{QuotaConfidence, QuotaIncident};

    fn identity(session_id: &str) -> SessionEvidenceIdentity {
        SessionEvidenceIdentity {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
        }
    }

    fn incident(ts_ms: i64, limit_kind: QuotaLimitKind, model: &str) -> QuotaIncident {
        QuotaIncident {
            ts_ms,
            limit_kind,
            severity: QuotaHitSeverity::HardHit,
            model: Some(model.to_owned()),
            reset_ts_ms: None,
            utilization_pct: None,
            confidence: QuotaConfidence::Observed,
        }
    }

    #[test]
    fn no_quota_evidence_in_any_state_is_the_one_not_assessed_condition() {
        let mut accumulator = QuotaPressureAccumulator::default();
        accumulator.observe_session(&identity("unsupported"), &EvidenceValue::Unsupported);
        accumulator.observe_session(
            &identity("complete-empty"),
            &EvidenceValue::Complete(SessionQuotaEvidence::default()),
        );
        accumulator.observe_session(
            &identity("partial-empty"),
            &EvidenceValue::Partial {
                observed: SessionQuotaEvidence::default(),
                reason: crate::analysis::CoverageReason::MalformedRecord,
            },
        );

        assert_eq!(accumulator.finish(), QuotaPressureSection::NotAssessed);
    }

    #[test]
    fn observed_incidents_are_deduplicated_and_fully_reported() {
        let mut accumulator = QuotaPressureAccumulator::default();
        let duplicate = incident(100, QuotaLimitKind::RollingWindow, "model-a");
        accumulator.observe_session(
            &identity("s1"),
            &EvidenceValue::Complete(SessionQuotaEvidence {
                incidents: vec![
                    duplicate.clone(),
                    duplicate,
                    incident(250, QuotaLimitKind::Weekly, "model-b"),
                ],
            }),
        );
        // Partial evidence with observed incidents still yields findings.
        accumulator.observe_session(
            &identity("s2"),
            &EvidenceValue::Partial {
                observed: SessionQuotaEvidence {
                    incidents: vec![incident(400, QuotaLimitKind::RateLimit, "model-a")],
                },
                reason: crate::analysis::CoverageReason::IncompleteTail,
            },
        );

        let QuotaPressureSection::Findings(findings) = accumulator.finish() else {
            panic!("expected findings");
        };
        assert_eq!(findings.total_hits, 3);
        assert_eq!(
            findings.hits_by_limit_kind,
            BTreeMap::from([
                (QuotaLimitKind::RollingWindow, 1),
                (QuotaLimitKind::Weekly, 1),
                (QuotaLimitKind::RateLimit, 1),
            ])
        );
        assert_eq!(findings.hard_hits, 3);
        assert_eq!(findings.warnings, 0);
        assert_eq!(findings.affected_session_count, 2);
        assert_eq!(
            findings
                .affected_session_examples
                .iter()
                .map(|example| example.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["s1", "s2"]
        );
        assert_eq!(
            findings.affected_models,
            BTreeSet::from(["model-a".to_owned(), "model-b".to_owned()])
        );
        assert!(!findings.affected_models_truncated);
        assert_eq!(findings.first_observed_ts_ms, 100);
        assert_eq!(findings.last_observed_ts_ms, 400);
        assert_eq!(findings.observed_times_ms, vec![100, 250, 400]);
    }

    #[test]
    fn a_relogged_incident_with_shifted_metadata_counts_once() {
        // The dedup key is (time, limit kind, severity, model).
        // Fields outside the key must not split one incident in two.
        // QuotaConfidence has one variant, so it cannot vary here.
        let mut accumulator = QuotaPressureAccumulator::default();
        let first = incident(100, QuotaLimitKind::RollingWindow, "model-a");
        let mut relogged = first.clone();
        relogged.reset_ts_ms = Some(9_999);
        relogged.utilization_pct = Some(97);
        accumulator.observe_session(
            &identity("s1"),
            &EvidenceValue::Complete(SessionQuotaEvidence {
                incidents: vec![first, relogged],
            }),
        );

        let QuotaPressureSection::Findings(findings) = accumulator.finish() else {
            panic!("expected findings");
        };
        assert_eq!(findings.total_hits, 1);
        assert_eq!(
            findings.hits_by_limit_kind,
            BTreeMap::from([(QuotaLimitKind::RollingWindow, 1)])
        );
        assert_eq!(findings.hard_hits, 1);
        assert_eq!(findings.observed_times_ms, vec![100]);
    }

    #[test]
    fn reported_collections_stay_bounded() {
        let mut accumulator = QuotaPressureAccumulator::default();
        for index in 0..(MAX_QUOTA_AFFECTED_MODELS + 4) {
            let index = i64::try_from(index).unwrap();
            accumulator.observe_session(
                &identity(&format!("s{index}")),
                &EvidenceValue::Complete(SessionQuotaEvidence {
                    incidents: vec![incident(
                        index,
                        QuotaLimitKind::ModelSpecific,
                        &format!("model-{index}"),
                    )],
                }),
            );
        }

        let QuotaPressureSection::Findings(findings) = accumulator.finish() else {
            panic!("expected findings");
        };
        assert_eq!(
            findings.affected_session_examples.len(),
            MAX_QUOTA_SESSION_EXAMPLES
        );
        assert_eq!(findings.affected_models.len(), MAX_QUOTA_AFFECTED_MODELS);
        assert!(findings.affected_models_truncated);
        assert_eq!(findings.observed_times_ms.len(), MAX_QUOTA_OBSERVED_TIMES);
        assert_eq!(
            findings.affected_session_count,
            u64::try_from(MAX_QUOTA_AFFECTED_MODELS + 4).unwrap()
        );
    }
}
