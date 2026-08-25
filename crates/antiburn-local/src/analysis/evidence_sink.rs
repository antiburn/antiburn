// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::collections::BTreeMap;

#[cfg(debug_assertions)]
use crate::analysis::evidence::UnfinishedGroup;
use crate::analysis::evidence::{
    ContextEvidence, CoverageReason, EvidenceCoverage, EvidenceSource, EvidenceValue,
    OrderingObservation, ParseDiagnostics, SessionEvidence, SessionEvidenceIdentity,
    SessionProvenance, SourceAcceptance, SourceCapabilities, SourceKind,
};
use crate::analysis::interface::{NormalizedRecord, RecordSink, SessionSummary, VisitOutcome};
use crate::analysis::metrics_sink::SessionMetricsAccumulator;
use crate::analysis::{
    ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION, SessionMetrics,
};

pub struct SessionEvidenceAccumulator {
    identity: SessionEvidenceIdentity,
    capabilities: SourceCapabilities,
    source_kind: SourceKind,
    source_acceptance: SourceAcceptance,
    ordering: OrderingObservation,
    diagnostics: ParseDiagnostics,
    max_request_context_tokens: u64,
    coverage_reason: Option<CoverageReason>,
    last_ts_ms: Option<i64>,
    summary_observed: bool,
}

impl SessionEvidenceAccumulator {
    pub fn new(source: EvidenceSource) -> Self {
        Self {
            identity: SessionEvidenceIdentity::new(&source.agent, &source.session_id),
            capabilities: source.capabilities,
            source_kind: source.kind,
            source_acceptance: SourceAcceptance::NotObserved,
            ordering: OrderingObservation::Monotonic,
            diagnostics: ParseDiagnostics {
                records_observed: 0,
                records_unusable: 0,
                unusable_reasons: BTreeMap::new(),
            },
            max_request_context_tokens: 0,
            coverage_reason: None,
            last_ts_ms: None,
            summary_observed: false,
        }
    }

    /// Folds one record without taking it.
    pub fn observe(&mut self, record: &NormalizedRecord) {
        self.diagnostics.records_observed = self.diagnostics.records_observed.saturating_add(1);
        match record {
            NormalizedRecord::MetricsEvent(event) => {
                self.max_request_context_tokens = self
                    .max_request_context_tokens
                    .max(event.usage.context_tokens());
                if let Some(timestamp) = event.ts_ms {
                    if self.last_ts_ms.is_some_and(|last| timestamp < last) {
                        self.ordering = OrderingObservation::OutOfOrder;
                    }
                    self.last_ts_ms = Some(timestamp);
                }
            }
            NormalizedRecord::Unusable(reason) => {
                let reason = CoverageReason::from(*reason);
                self.diagnostics.records_unusable =
                    self.diagnostics.records_unusable.saturating_add(1);
                let count = self.diagnostics.unusable_reasons.entry(reason).or_default();
                *count = count.saturating_add(1);
                self.set_coverage_reason(reason);
            }
        }
    }

    /// Folds the end-of-stream facts without taking them.
    pub fn observe_summary(&mut self, summary: &SessionSummary) {
        self.capabilities.cache_write_tokens = summary.cache_write_tokens_available;
        self.summary_observed = true;
    }

    /// Attaches the source outcome after the adapter returns.
    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        if matches!(outcome, VisitOutcome::AcceptedPrefix { .. }) {
            self.set_coverage_reason(CoverageReason::PinnedPrefix);
        }
        self.source_acceptance = SourceAcceptance::from(outcome);
    }

    pub fn evidence(&self) -> SessionEvidence {
        let context = ContextEvidence {
            max_request_context_tokens: self.max_request_context_tokens,
        };
        let context = if !self.capabilities.request_context_tokens {
            EvidenceValue::Unsupported
        } else if let Some(reason) = self.coverage_reason {
            EvidenceValue::Partial {
                observed: context,
                reason,
            }
        } else {
            EvidenceValue::Complete(context)
        };
        let coverage = self
            .coverage_reason
            .map_or(EvidenceCoverage::Complete, EvidenceCoverage::Partial);

        SessionEvidence {
            schema_revision: EVIDENCE_SCHEMA_REVISION,
            identity: self.identity.clone(),
            context,
            capabilities: self.capabilities,
            coverage,
            provenance: SessionProvenance {
                parser_revision: PARSER_REVISION,
                analyzer_revision: ANALYZER_REVISION,
                evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                source_kind: self.source_kind,
                source_acceptance: self.source_acceptance,
                ordering: self.ordering,
            },
            diagnostics: self.diagnostics.clone(),
            #[cfg(debug_assertions)]
            time_range: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            eligibility: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            models: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            tools: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            context_sources: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            subagents: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            cache: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            compactions: EvidenceValue::<UnfinishedGroup>::Unimplemented,
            #[cfg(debug_assertions)]
            quota_incidents: EvidenceValue::<UnfinishedGroup>::Unimplemented,
        }
    }

    fn set_coverage_reason(&mut self, reason: CoverageReason) {
        if self.coverage_reason.is_none() {
            self.coverage_reason = Some(reason);
        }
    }

    fn can_publish(&self) -> bool {
        self.summary_observed
            && !matches!(
                self.source_acceptance,
                SourceAcceptance::NotObserved | SourceAcceptance::SourceChanged
            )
    }
}

impl RecordSink for SessionEvidenceAccumulator {
    fn record(&mut self, record: NormalizedRecord) {
        self.observe(&record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.observe_summary(&summary);
    }
}

pub struct CompositeSink {
    metrics: SessionMetricsAccumulator,
    evidence: SessionEvidenceAccumulator,
}

impl CompositeSink {
    pub fn new(metrics: SessionMetricsAccumulator, evidence: SessionEvidenceAccumulator) -> Self {
        Self { metrics, evidence }
    }

    pub fn metrics(&self) -> Option<SessionMetrics> {
        self.evidence.can_publish().then(|| self.metrics.metrics())
    }

    pub fn evidence(&self) -> Option<SessionEvidence> {
        self.evidence
            .can_publish()
            .then(|| self.evidence.evidence())
    }

    pub fn observe_source_outcome(&mut self, outcome: VisitOutcome) {
        self.evidence.observe_source_outcome(outcome);
    }

    pub fn into_parts(self) -> Option<(SessionMetricsAccumulator, SessionEvidenceAccumulator)> {
        self.evidence
            .can_publish()
            .then_some((self.metrics, self.evidence))
    }
}

impl RecordSink for CompositeSink {
    fn record(&mut self, record: NormalizedRecord) {
        self.evidence.observe(&record);
        self.metrics.record(record);
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.evidence.observe_summary(&summary);
        self.metrics.finish(summary);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::analysis::model::NormalizedEvent;
    use crate::analysis::{EventSource, PartialReason, Role, SourceChangedReason, Usage};

    #[test]
    fn context_depth_is_complete_zero_without_a_usage_bearing_record() {
        let mut accumulator = accumulator(true);
        accumulator.finish(SessionSummary::default());

        assert_eq!(
            accumulator.evidence().context,
            EvidenceValue::Complete(ContextEvidence {
                max_request_context_tokens: 0,
            })
        );
    }

    #[test]
    fn context_depth_is_the_maximum_across_every_event_source() {
        let mut accumulator = accumulator(true);
        accumulator.record(metric_record(EventSource::Parent, 5, Some(1)));
        accumulator.record(metric_record(EventSource::Subagent, 12, Some(2)));
        accumulator.finish(SessionSummary::default());

        assert_eq!(
            accumulator.evidence().context,
            EvidenceValue::Complete(ContextEvidence {
                max_request_context_tokens: 12,
            })
        );
    }

    #[test]
    fn malformed_record_downgrades_context_to_partial() {
        assert_partial_after_unusable(
            PartialReason::MalformedRecord,
            CoverageReason::MalformedRecord,
        );
    }

    #[test]
    fn oversized_record_downgrades_context_to_partial() {
        assert_partial_after_unusable(PartialReason::Oversized, CoverageReason::Oversized);
    }

    #[test]
    fn the_first_unusable_reason_is_the_reported_reason() {
        let mut accumulator = accumulator(true);
        accumulator.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        accumulator.record(NormalizedRecord::Unusable(PartialReason::Oversized));
        accumulator.finish(SessionSummary::default());
        let evidence = accumulator.evidence();

        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::MalformedRecord)
        );
        assert_eq!(
            evidence.diagnostics.unusable_reasons,
            BTreeMap::from([
                (CoverageReason::MalformedRecord, 1),
                (CoverageReason::Oversized, 1),
            ])
        );
        let first = serde_json::to_string(&evidence.diagnostics.unusable_reasons).unwrap();
        let second = serde_json::to_string(&evidence.diagnostics.unusable_reasons).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn unsupported_capability_yields_unsupported_not_zero() {
        let mut accumulator = accumulator(false);
        accumulator.record(metric_record(EventSource::Parent, 7, Some(1)));
        accumulator.finish(SessionSummary::default());

        assert_eq!(accumulator.evidence().context, EvidenceValue::Unsupported);
    }

    #[test]
    fn diagnostics_counters_and_reason_map_stay_bounded() {
        let mut accumulator = accumulator(true);
        let reasons = [
            PartialReason::Oversized,
            PartialReason::MalformedRecord,
            PartialReason::IncompleteTail,
            PartialReason::Cancelled,
            PartialReason::ReadFailed,
            PartialReason::UnrecognizedRecordType,
        ];
        for _ in 0..20 {
            for reason in reasons {
                accumulator.record(NormalizedRecord::Unusable(reason));
            }
        }
        accumulator.finish(SessionSummary {
            cache_write_tokens_available: true,
            ..SessionSummary::default()
        });
        let evidence = accumulator.evidence();

        assert!(evidence.diagnostics.unusable_reasons.len() <= 7);
        assert_eq!(evidence.diagnostics.records_observed, 120);
        assert_eq!(evidence.diagnostics.records_unusable, 120);
        assert!(evidence.capabilities.cache_write_tokens);
    }

    #[test]
    fn source_outcome_maps_every_visit_outcome() {
        let outcomes = [
            (VisitOutcome::Unvalidated, SourceAcceptance::Unvalidated),
            (VisitOutcome::AcceptedFull, SourceAcceptance::AcceptedFull),
            (
                VisitOutcome::AcceptedPrefix { boundary: 4096 },
                SourceAcceptance::AcceptedPrefix { boundary: 4096 },
            ),
            (
                VisitOutcome::SourceChanged(SourceChangedReason::IdentityMismatch),
                SourceAcceptance::SourceChanged,
            ),
        ];
        assert_eq!(
            accumulator(true).evidence().provenance.source_acceptance,
            SourceAcceptance::NotObserved
        );

        for (outcome, expected) in outcomes {
            let mut accumulator = accumulator(true);
            accumulator.observe_source_outcome(outcome);
            assert_eq!(
                accumulator.evidence().provenance.source_acceptance,
                expected
            );
        }

        let mut accepted = accumulator(true);
        accepted.observe_source_outcome(VisitOutcome::AcceptedFull);
        assert_eq!(accepted.evidence().coverage, EvidenceCoverage::Complete);
    }

    #[test]
    fn accepted_prefix_downgrades_coverage_to_partial() {
        let mut prefix_accumulator = accumulator(true);
        prefix_accumulator.record(metric_record(EventSource::Parent, 7, Some(1)));
        prefix_accumulator.observe_source_outcome(VisitOutcome::AcceptedPrefix { boundary: 4096 });
        let evidence = prefix_accumulator.evidence();

        assert_eq!(
            evidence.coverage,
            EvidenceCoverage::Partial(CoverageReason::PinnedPrefix)
        );
        assert_eq!(
            evidence.context,
            EvidenceValue::Partial {
                observed: ContextEvidence {
                    max_request_context_tokens: 7,
                },
                reason: CoverageReason::PinnedPrefix,
            }
        );

        let mut earlier_reason = accumulator(true);
        earlier_reason.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
        earlier_reason.observe_source_outcome(VisitOutcome::AcceptedPrefix { boundary: 4096 });
        assert_eq!(
            earlier_reason.evidence().coverage,
            EvidenceCoverage::Partial(CoverageReason::MalformedRecord)
        );
    }

    #[test]
    fn source_changed_without_summary_keeps_projections_unpublished() {
        let mut composite = CompositeSink::new(
            SessionMetricsAccumulator::new("claude", "s1"),
            accumulator(true),
        );
        composite.record(metric_record(EventSource::Parent, 7, Some(1)));
        composite.observe_source_outcome(VisitOutcome::SourceChanged(
            SourceChangedReason::IdentityMismatch,
        ));

        assert!(composite.metrics().is_none());
        assert!(composite.evidence().is_none());
        assert!(composite.into_parts().is_none());
    }

    fn accumulator(request_context_tokens: bool) -> SessionEvidenceAccumulator {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities {
                request_context_tokens,
                cache_write_tokens: false,
            },
        })
    }

    fn metric_record(
        source: EventSource,
        context_tokens: u64,
        ts_ms: Option<i64>,
    ) -> NormalizedRecord {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.source = source;
        event.ts_ms = ts_ms;
        event.usage = Usage {
            input_tokens: context_tokens,
            ..Usage::default()
        };
        NormalizedRecord::MetricsEvent(Box::new(event))
    }

    fn assert_partial_after_unusable(partial: PartialReason, reason: CoverageReason) {
        let mut accumulator = accumulator(true);
        accumulator.record(metric_record(EventSource::Parent, 7, Some(1)));
        accumulator.record(NormalizedRecord::Unusable(partial));
        accumulator.finish(SessionSummary::default());
        let evidence = accumulator.evidence();

        assert_eq!(
            evidence.context,
            EvidenceValue::Partial {
                observed: ContextEvidence {
                    max_request_context_tokens: 7,
                },
                reason,
            }
        );
        assert_eq!(evidence.coverage, EvidenceCoverage::Partial(reason));
    }
}
