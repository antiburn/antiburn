//! Provider-incidents section over transcript-attributable incidents.
//!
//! The section stays outside the nine-category detector contract
//! (FR-15). Its only input is the `provider_incidents` evidence group.
//! It calls no provider endpoint and reads no account-level limit
//! state. The section is not assessed exactly when the transcripts
//! carry no provider incident evidence — one condition, not a matrix.
//! Presence read from partial evidence is trustworthy, so partial
//! evidence with observed incidents still produces findings.
//!
//! This section is a sibling of [`super::quota`], not a rename of it:
//! `quota_incidents` stays scoped to user-allocation limits, while this
//! group carries provider-side failures the user's own usage did not
//! cause (Codex's `server_overloaded`).

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{
    EvidenceValue, ProviderIncidentKind, SessionEvidenceIdentity, SessionProviderEvidence,
};

use super::report::SessionExample;

/// Caps the reported affected-session examples.
pub const MAX_PROVIDER_SESSION_EXAMPLES: usize = 3;
/// Caps the reported affected-model names.
pub const MAX_PROVIDER_AFFECTED_MODELS: usize = 16;
/// Caps the reported example observation times.
pub const MAX_PROVIDER_OBSERVED_TIMES: usize = 8;

/// One report-level provider-incidents result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderIncidentsSection {
    /// The transcripts carry no provider incident evidence.
    NotAssessed,
    Findings(ProviderIncidentFindings),
}

/// Bounded summary of deduplicated transcript provider incidents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIncidentFindings {
    /// Deduplicated hit count per incident kind.
    pub hits_by_kind: BTreeMap<ProviderIncidentKind, u64>,
    /// Total deduplicated hits across all kinds.
    pub total_hits: u64,
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

/// Folds per-session provider-incident evidence into the bounded section state.
#[derive(Debug, Default)]
pub(crate) struct ProviderIncidentsAccumulator {
    findings: Option<ProviderIncidentFindings>,
}

impl ProviderIncidentsAccumulator {
    /// Observes one cohort session. Incidents are deduplicated within
    /// the session on time, kind, and model, so a retried capacity
    /// error logged twice counts once. The same incident observed by
    /// two sessions counts per session, because each session pays the
    /// interruption.
    pub(crate) fn observe_session(
        &mut self,
        identity: &SessionEvidenceIdentity,
        provider: &EvidenceValue<SessionProviderEvidence>,
    ) {
        let incidents = match provider {
            EvidenceValue::Unsupported => return,
            EvidenceValue::Partial { observed, .. } => &observed.incidents,
            EvidenceValue::Complete(observed) => &observed.incidents,
        };
        // The key projects the incident identity — time, kind, and model.
        // The set is transient; CH-009 owns the per-session incident cap
        // that bounds the input collection.
        let deduplicated: BTreeSet<_> = incidents
            .iter()
            .map(|incident| (incident.ts_ms, incident.kind, incident.model.as_deref()))
            .collect();
        if deduplicated.is_empty() {
            return;
        }

        let findings = self
            .findings
            .get_or_insert_with(|| ProviderIncidentFindings {
                hits_by_kind: BTreeMap::new(),
                total_hits: 0,
                affected_session_count: 0,
                affected_session_examples: Vec::new(),
                affected_models: BTreeSet::new(),
                affected_models_truncated: false,
                first_observed_ts_ms: i64::MAX,
                last_observed_ts_ms: i64::MIN,
                observed_times_ms: Vec::new(),
            });
        findings.affected_session_count += 1;
        if findings.affected_session_examples.len() < MAX_PROVIDER_SESSION_EXAMPLES {
            findings.affected_session_examples.push(SessionExample {
                agent: identity.agent.clone(),
                session_id: identity.session_id.clone(),
            });
        }
        for (ts_ms, kind, model) in deduplicated {
            findings.total_hits += 1;
            *findings.hits_by_kind.entry(kind).or_default() += 1;
            if let Some(model) = model {
                if findings.affected_models.contains(model) {
                    // Already retained.
                } else if findings.affected_models.len() < MAX_PROVIDER_AFFECTED_MODELS {
                    findings.affected_models.insert(model.to_owned());
                } else {
                    findings.affected_models_truncated = true;
                }
            }
            findings.first_observed_ts_ms = findings.first_observed_ts_ms.min(ts_ms);
            findings.last_observed_ts_ms = findings.last_observed_ts_ms.max(ts_ms);
            if findings.observed_times_ms.len() < MAX_PROVIDER_OBSERVED_TIMES {
                findings.observed_times_ms.push(ts_ms);
            }
        }
    }

    /// Finalizes the section. No observed incident anywhere means the
    /// section is not assessed — the one condition of FR-15.
    pub(crate) fn finish(self) -> ProviderIncidentsSection {
        match self.findings {
            None => ProviderIncidentsSection::NotAssessed,
            Some(findings) => ProviderIncidentsSection::Findings(findings),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::ProviderIncident;

    fn identity(session_id: &str) -> SessionEvidenceIdentity {
        SessionEvidenceIdentity {
            agent: "codex".to_owned(),
            session_id: session_id.to_owned(),
        }
    }

    fn incident(ts_ms: i64, kind: ProviderIncidentKind, model: &str) -> ProviderIncident {
        ProviderIncident {
            ts_ms,
            kind,
            model: Some(model.to_owned()),
        }
    }

    #[test]
    fn no_provider_evidence_in_any_state_is_the_one_not_assessed_condition() {
        let mut accumulator = ProviderIncidentsAccumulator::default();
        accumulator.observe_session(&identity("unsupported"), &EvidenceValue::Unsupported);
        accumulator.observe_session(
            &identity("complete-empty"),
            &EvidenceValue::Complete(SessionProviderEvidence::default()),
        );
        accumulator.observe_session(
            &identity("partial-empty"),
            &EvidenceValue::Partial {
                observed: SessionProviderEvidence::default(),
                reason: crate::analysis::CoverageReason::MalformedRecord,
            },
        );

        assert_eq!(accumulator.finish(), ProviderIncidentsSection::NotAssessed);
    }

    #[test]
    fn observed_incidents_are_deduplicated_and_fully_reported() {
        let mut accumulator = ProviderIncidentsAccumulator::default();
        let duplicate = incident(100, ProviderIncidentKind::Capacity, "model-a");
        accumulator.observe_session(
            &identity("s1"),
            &EvidenceValue::Complete(SessionProviderEvidence {
                incidents: vec![
                    duplicate.clone(),
                    duplicate,
                    incident(250, ProviderIncidentKind::Capacity, "model-b"),
                ],
            }),
        );
        // Partial evidence with observed incidents still yields findings.
        accumulator.observe_session(
            &identity("s2"),
            &EvidenceValue::Partial {
                observed: SessionProviderEvidence {
                    incidents: vec![incident(400, ProviderIncidentKind::Capacity, "model-a")],
                },
                reason: crate::analysis::CoverageReason::IncompleteTail,
            },
        );

        let ProviderIncidentsSection::Findings(findings) = accumulator.finish() else {
            panic!("expected findings");
        };
        assert_eq!(findings.total_hits, 3);
        assert_eq!(
            findings.hits_by_kind,
            BTreeMap::from([(ProviderIncidentKind::Capacity, 3)])
        );
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

    /// Two sessions each carry a distinct mix of kinds. `hits_by_kind`
    /// locks the per-kind counts across both sessions.
    #[test]
    fn mixed_kinds_across_two_sessions_lock_the_per_kind_counts() {
        let mut accumulator = ProviderIncidentsAccumulator::default();
        accumulator.observe_session(
            &identity("s1"),
            &EvidenceValue::Complete(SessionProviderEvidence {
                incidents: vec![
                    incident(100, ProviderIncidentKind::Capacity, "model-a"),
                    incident(200, ProviderIncidentKind::ServerError, "model-a"),
                ],
            }),
        );
        accumulator.observe_session(
            &identity("s2"),
            &EvidenceValue::Complete(SessionProviderEvidence {
                incidents: vec![
                    incident(300, ProviderIncidentKind::ServerError, "model-b"),
                    incident(400, ProviderIncidentKind::Connection, "model-b"),
                    incident(500, ProviderIncidentKind::Connection, "model-b"),
                ],
            }),
        );

        let ProviderIncidentsSection::Findings(findings) = accumulator.finish() else {
            panic!("expected findings");
        };
        assert_eq!(findings.total_hits, 5);
        assert_eq!(
            findings.hits_by_kind,
            BTreeMap::from([
                (ProviderIncidentKind::Capacity, 1),
                (ProviderIncidentKind::ServerError, 2),
                (ProviderIncidentKind::Connection, 2),
            ])
        );
        assert_eq!(findings.affected_session_count, 2);
    }

    #[test]
    fn reported_collections_stay_bounded() {
        let mut accumulator = ProviderIncidentsAccumulator::default();
        for index in 0..(MAX_PROVIDER_AFFECTED_MODELS + 4) {
            let index = i64::try_from(index).unwrap();
            accumulator.observe_session(
                &identity(&format!("s{index}")),
                &EvidenceValue::Complete(SessionProviderEvidence {
                    incidents: vec![incident(
                        index,
                        ProviderIncidentKind::Capacity,
                        &format!("model-{index}"),
                    )],
                }),
            );
        }

        let ProviderIncidentsSection::Findings(findings) = accumulator.finish() else {
            panic!("expected findings");
        };
        assert_eq!(
            findings.affected_session_examples.len(),
            MAX_PROVIDER_SESSION_EXAMPLES
        );
        assert_eq!(findings.affected_models.len(), MAX_PROVIDER_AFFECTED_MODELS);
        assert!(findings.affected_models_truncated);
        assert_eq!(
            findings.observed_times_ms.len(),
            MAX_PROVIDER_OBSERVED_TIMES
        );
        assert_eq!(
            findings.affected_session_count,
            u64::try_from(MAX_PROVIDER_AFFECTED_MODELS + 4).unwrap()
        );
    }
}
