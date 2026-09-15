//! Which provider or quota incidents are new since a session's previously
//! published evidence.
//!
//! [`newly_reportable`] is a pure function. It needs no store and no
//! `tauri::AppHandle`. `insights_worker::apply_outcome` calls it on every
//! publish. Tests can call it directly with plain [`SessionEvidence`]
//! values. This module builds with or without the `analytics` feature:
//! `apply_outcome` always computes the newly reportable incidents. Only the
//! enabled build's `record_provider_incidents_ingested` decides whether to
//! send them.

use std::collections::{BTreeMap, BTreeSet};

use antiburn_local::analysis::{
    EvidenceValue, ProviderIncident, ProviderIncidentKind, QuotaIncident, QuotaLimitKind,
    SessionEvidence, SessionProviderEvidence, SessionQuotaEvidence,
};

/// How long an incident stays reportable after it happens.
///
/// This bounds first-install backfill. It also bounds a parser
/// reclassification of old records, and an unreadable previous evidence
/// blob. The event's own capture time stays within two hours of the
/// incident it reports.
pub const INGESTED_INCIDENT_FRESHNESS_MS: i64 = 2 * 60 * 60 * 1000;

/// One kind of incident this event may report.
///
/// This is a merged vocabulary. It is not the evidence crate's own
/// incident-kind enums. It combines the three provider kinds and the two
/// in-scope quota kinds into one closed set. This set is the value
/// `antiburn.provider_incidents_ingested`'s `detail` field carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IngestedIncidentKind {
    Capacity,
    ServerError,
    Connection,
    RateLimit,
    UsageLimit,
}

impl IngestedIncidentKind {
    /// The closed analytics label for this kind.
    ///
    /// This matches the vocabulary `provider_incident_kind_label` and
    /// `quota_limit_kind_label` already use. The same kind reports the same
    /// string whether Insights or this event names it.
    ///
    /// Only the enabled build's `record_provider_incidents_ingested` calls
    /// this method. A disabled build needs only the kind, not its string
    /// form, so this method is gated the same way.
    #[cfg(feature = "analytics")]
    pub fn label(self) -> &'static str {
        match self {
            Self::Capacity => "capacity",
            Self::ServerError => "server_error",
            Self::Connection => "connection",
            Self::RateLimit => "rate_limit",
            Self::UsageLimit => "usage_limit",
        }
    }
}

/// One `(kind, count)` pair to report for a published session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestedIncidents {
    pub counts: BTreeMap<IngestedIncidentKind, usize>,
}

impl IngestedIncidents {
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
}

/// The provider incidents a session's evidence carries, or `None` when the
/// group is `Unsupported`.
fn provider_incidents(evidence: &SessionEvidence) -> Option<&[ProviderIncident]> {
    match &evidence.provider_incidents {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial {
            observed: SessionProviderEvidence { incidents },
            ..
        }
        | EvidenceValue::Complete(SessionProviderEvidence { incidents }) => Some(incidents),
    }
}

/// The quota incidents a session's evidence carries, or `None` when the
/// group is `Unsupported`.
fn quota_incidents(evidence: &SessionEvidence) -> Option<&[QuotaIncident]> {
    match &evidence.quota_incidents {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial {
            observed: SessionQuotaEvidence { incidents },
            ..
        }
        | EvidenceValue::Complete(SessionQuotaEvidence { incidents }) => Some(incidents),
    }
}

/// Maps a quota limit kind into this event's vocabulary. Returns `None` for
/// a kind outside it. Today, every kind except `RateLimit` and `UsageLimit`
/// is outside it.
fn quota_kind(kind: QuotaLimitKind) -> Option<IngestedIncidentKind> {
    match kind {
        QuotaLimitKind::RateLimit => Some(IngestedIncidentKind::RateLimit),
        QuotaLimitKind::UsageLimit => Some(IngestedIncidentKind::UsageLimit),
        QuotaLimitKind::RollingWindow
        | QuotaLimitKind::Weekly
        | QuotaLimitKind::ModelSpecific
        | QuotaLimitKind::WeightedUsage => None,
    }
}

fn provider_kind(kind: ProviderIncidentKind) -> IngestedIncidentKind {
    match kind {
        ProviderIncidentKind::Capacity => IngestedIncidentKind::Capacity,
        ProviderIncidentKind::ServerError => IngestedIncidentKind::ServerError,
        ProviderIncidentKind::Connection => IngestedIncidentKind::Connection,
    }
}

/// Every `(ts_ms, kind)` pair one evidence blob's provider and quota
/// incident groups carry. This ignores `model`. So a re-derived incident
/// with the same timestamp and kind is the same incident, even when its
/// attribution details differ.
fn incident_pairs(evidence: &SessionEvidence) -> BTreeSet<(i64, IngestedIncidentKind)> {
    let mut pairs = BTreeSet::new();
    if let Some(incidents) = provider_incidents(evidence) {
        pairs.extend(
            incidents
                .iter()
                .map(|incident| (incident.ts_ms, provider_kind(incident.kind))),
        );
    }
    if let Some(incidents) = quota_incidents(evidence) {
        pairs.extend(
            incidents
                .iter()
                .filter_map(|incident| Some((incident.ts_ms, quota_kind(incident.limit_kind)?))),
        );
    }
    pairs
}

/// The incidents `current` carries that `previous` did not. Each returned
/// incident also happened within [`INGESTED_INCIDENT_FRESHNESS_MS`] of
/// `now_ms`.
///
/// `previous` is `None` for a session's first publish. It is also `None`
/// when the earlier evidence blob could not be read back, for example after
/// an older schema revision. Either way, this function then reports every
/// fresh incident `current` carries, exactly as a first publish would. A
/// pair present in `previous` is never new, even after it ages out of the
/// freshness window. This function only adds incidents. It never re-checks
/// what an earlier publish already had the chance to report.
pub fn newly_reportable(
    previous: Option<&SessionEvidence>,
    current: &SessionEvidence,
    now_ms: i64,
) -> IngestedIncidents {
    let previously_seen = previous.map(incident_pairs).unwrap_or_default();
    let mut result = IngestedIncidents::default();
    for (ts_ms, kind) in incident_pairs(current) {
        if previously_seen.contains(&(ts_ms, kind)) {
            continue;
        }
        let age_ms = now_ms - ts_ms;
        if !(0..=INGESTED_INCIDENT_FRESHNESS_MS).contains(&age_ms) {
            continue;
        }
        *result.counts.entry(kind).or_insert(0) += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use antiburn_local::analysis::{
        CoverageReason, EvidenceSource, QuotaConfidence, QuotaHitSeverity,
        SessionEvidenceAccumulator, SourceCapabilities, SourceKind, TurnFacts,
    };

    use super::*;

    fn evidence_with(
        provider: EvidenceValue<SessionProviderEvidence>,
        quota: EvidenceValue<SessionQuotaEvidence>,
    ) -> SessionEvidence {
        let mut evidence = empty_evidence();
        evidence.provider_incidents = provider;
        evidence.quota_incidents = quota;
        evidence
    }

    /// A minimal, otherwise-empty [`SessionEvidence`], built the same way
    /// `evidence.rs`'s own tests do: through the accumulator, so this test
    /// module never has to hand-build the struct's many other fields.
    fn empty_evidence() -> SessionEvidence {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default())
    }

    fn provider(ts_ms: i64, kind: ProviderIncidentKind) -> ProviderIncident {
        ProviderIncident {
            ts_ms,
            kind,
            model: None,
        }
    }

    fn quota(ts_ms: i64, limit_kind: QuotaLimitKind) -> QuotaIncident {
        QuotaIncident {
            ts_ms,
            limit_kind,
            severity: QuotaHitSeverity::HardHit,
            model: None,
            reset_ts_ms: None,
            utilization_pct: None,
            confidence: QuotaConfidence::Observed,
        }
    }

    fn complete_provider(
        incidents: Vec<ProviderIncident>,
    ) -> EvidenceValue<SessionProviderEvidence> {
        EvidenceValue::Complete(SessionProviderEvidence { incidents })
    }

    fn complete_quota(incidents: Vec<QuotaIncident>) -> EvidenceValue<SessionQuotaEvidence> {
        EvidenceValue::Complete(SessionQuotaEvidence { incidents })
    }

    /// Test 1: no previous evidence, one fresh capacity incident.
    #[test]
    fn a_fresh_incident_with_no_previous_evidence_is_reportable() {
        let current = evidence_with(
            complete_provider(vec![provider(1_000, ProviderIncidentKind::Capacity)]),
            EvidenceValue::Unsupported,
        );
        let result = newly_reportable(None, &current, 1_000);
        assert_eq!(result.counts.get(&IngestedIncidentKind::Capacity), Some(&1));
        assert_eq!(result.counts.len(), 1);
    }

    /// Test 2: previous evidence already carries the same `(ts_ms, kind)`.
    #[test]
    fn an_incident_already_in_previous_evidence_is_not_new() {
        let previous = evidence_with(
            complete_provider(vec![provider(1_000, ProviderIncidentKind::Capacity)]),
            EvidenceValue::Unsupported,
        );
        let current = previous.clone();
        let result = newly_reportable(Some(&previous), &current, 1_000);
        assert!(result.is_empty());
    }

    /// Test 3: previous has one incident; current adds a newer server error.
    #[test]
    fn an_appended_incident_reports_only_itself() {
        let previous = evidence_with(
            complete_provider(vec![provider(1_000, ProviderIncidentKind::Capacity)]),
            EvidenceValue::Unsupported,
        );
        let current = evidence_with(
            complete_provider(vec![
                provider(1_000, ProviderIncidentKind::Capacity),
                provider(2_000, ProviderIncidentKind::ServerError),
            ]),
            EvidenceValue::Unsupported,
        );
        let result = newly_reportable(Some(&previous), &current, 2_000);
        assert_eq!(
            result.counts.get(&IngestedIncidentKind::ServerError),
            Some(&1)
        );
        assert_eq!(result.counts.len(), 1);
    }

    /// Test 4: an incident three hours old, with no previous evidence, is stale.
    #[test]
    fn a_stale_incident_is_not_reportable() {
        let current = evidence_with(
            complete_provider(vec![provider(1_000, ProviderIncidentKind::Capacity)]),
            EvidenceValue::Unsupported,
        );
        let three_hours_ms = 3 * 60 * 60 * 1000;
        let result = newly_reportable(None, &current, 1_000 + three_hours_ms);
        assert!(result.is_empty());
    }

    /// Test 5: a `ts_ms` in the future (clock skew) is dropped.
    #[test]
    fn a_future_incident_is_not_reportable() {
        let current = evidence_with(
            complete_provider(vec![provider(10_000, ProviderIncidentKind::Capacity)]),
            EvidenceValue::Unsupported,
        );
        let result = newly_reportable(None, &current, 1_000);
        assert!(result.is_empty());
    }

    /// Test 6: the same `ts_ms` with a different kind counts as a new incident.
    #[test]
    fn the_same_timestamp_with_a_different_kind_is_new() {
        let previous = evidence_with(
            complete_provider(vec![provider(1_000, ProviderIncidentKind::Capacity)]),
            EvidenceValue::Unsupported,
        );
        let current = evidence_with(
            complete_provider(vec![
                provider(1_000, ProviderIncidentKind::Capacity),
                provider(1_000, ProviderIncidentKind::ServerError),
            ]),
            EvidenceValue::Unsupported,
        );
        let result = newly_reportable(Some(&previous), &current, 1_000);
        assert_eq!(
            result.counts.get(&IngestedIncidentKind::ServerError),
            Some(&1)
        );
        assert_eq!(result.counts.len(), 1);
    }

    /// Test 7: `RateLimit` and `UsageLimit` map into the vocabulary; a
    /// `Weekly` quota incident is dropped.
    #[test]
    fn only_rate_limit_and_usage_limit_quota_kinds_map() {
        let current = evidence_with(
            EvidenceValue::Unsupported,
            complete_quota(vec![
                quota(1_000, QuotaLimitKind::RateLimit),
                quota(1_000, QuotaLimitKind::UsageLimit),
                quota(1_000, QuotaLimitKind::Weekly),
            ]),
        );
        let result = newly_reportable(None, &current, 1_000);
        assert_eq!(
            result.counts.get(&IngestedIncidentKind::RateLimit),
            Some(&1)
        );
        assert_eq!(
            result.counts.get(&IngestedIncidentKind::UsageLimit),
            Some(&1)
        );
        assert_eq!(result.counts.len(), 2);
    }

    /// Test 8: an `Unsupported` group contributes nothing.
    #[test]
    fn an_unsupported_group_contributes_nothing() {
        let current = evidence_with(EvidenceValue::Unsupported, EvidenceValue::Unsupported);
        let result = newly_reportable(None, &current, 1_000);
        assert!(result.is_empty());

        let partial = evidence_with(
            EvidenceValue::Partial {
                observed: SessionProviderEvidence {
                    incidents: vec![provider(1_000, ProviderIncidentKind::Capacity)],
                },
                reason: CoverageReason::Oversized,
            },
            EvidenceValue::Unsupported,
        );
        let result = newly_reportable(None, &partial, 1_000);
        assert_eq!(
            result.counts.get(&IngestedIncidentKind::Capacity),
            Some(&1),
            "a Partial group still contributes its observed incidents"
        );
    }
}
