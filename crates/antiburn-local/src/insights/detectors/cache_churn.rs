//! Cache Churn: per-thread repeated-context accounting, plus the legacy
//! churn-event rule under cache-write accounting.
//!
//! The primary rule reads `repeated_context`: paid context beyond
//! positive growth, summed over adjacent main-thread turn pairs (see
//! `evidence_query::query_repeated_context`). At or above
//! `catalogs.repeated_context_tokens_threshold`, that is a finding on
//! its own, under either accounting — cache-write (Anthropic) or
//! uncached-input (OpenAI).
//!
//! A second, sufficient condition stays for compatibility, so an
//! existing Claude finding never vanishes: an observed churn event —
//! idle expiry, model switching, or manual compaction — next to paid
//! cache writes, under cache-write accounting only (uncached-input
//! accounting has no `cache_creation_tokens` to read). A report never
//! distinguishes which of the two conditions fired.
//!
//! Provider-eviction estimates are not part of either rule: the
//! `provider_eviction` marker carries no estimate yet. The legacy rule
//! uses only user-visible churn events the transcript proves.
//!
//! Partial-evidence rules:
//! - Partial cache evidence still permits a finding. Repeated tokens at
//!   or above the threshold, or an observed churn event next to paid
//!   cache writes, both prove presence.
//! - Partial cache evidence prevents clean. A missed record may hide
//!   churn or understate repeated context.
//! - `repeated_context` `Unsupported` reports a contract gap: the source
//!   supports neither cache-write nor uncached-input accounting.

use crate::analysis::{EvidenceValue, RepeatedContextAccounting, SessionEvidence};

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    let Some(cache) = observed(&evidence.cache) else {
        return Observation::ContractIncomplete;
    };
    let repeated_context = match &cache.repeated_context {
        EvidenceValue::Unsupported => return Observation::ContractIncomplete,
        EvidenceValue::Partial { observed, .. } => observed,
        EvidenceValue::Complete(observed) => observed,
    };
    if repeated_context.repeated_tokens >= catalogs.repeated_context_tokens_threshold {
        return Observation::Finding;
    }
    let cache_write_accounting =
        repeated_context.accounting == RepeatedContextAccounting::CacheWrite;
    let churn_event = !cache.model_transitions.is_empty()
        || (cache.longest_idle_gap_ms > 0
            && cache.longest_idle_gap_ms >= catalogs.cache_idle_expiry_ms)
        || cache.user_controlled_churn.manual_compactions > 0;
    if cache_write_accounting && churn_event && cache.cache_creation_tokens > 0 {
        return Observation::Finding;
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{
        CacheEvidence, CoverageReason, EvidenceValue, ModelTransition, RepeatedContext,
        RepeatedContextAccounting,
    };

    fn edit_cache(
        evidence: &mut SessionEvidence,
        partial: bool,
        edit: impl FnOnce(&mut CacheEvidence),
    ) {
        let EvidenceValue::Complete(mut cache) = evidence.cache.clone() else {
            unreachable!()
        };
        edit(&mut cache);
        evidence.cache = if partial {
            EvidenceValue::Partial {
                observed: cache,
                reason: CoverageReason::MalformedRecord,
            }
        } else {
            EvidenceValue::Complete(cache)
        };
    }

    #[test]
    fn model_switch_with_paid_cache_writes_is_a_finding_even_from_partial_evidence() {
        for partial in [false, true] {
            let mut evidence = claude_evidence("churn");
            edit_cache(&mut evidence, partial, |cache| {
                cache.cache_creation_tokens = 5_000;
                cache.model_transitions.push(ModelTransition {
                    ts_ms: 10,
                    from_model: "model-a".to_owned(),
                    to_model: "model-b".to_owned(),
                });
            });

            assert_eq!(evaluate(&evidence, &ReportCatalogs::default()), {
                Observation::Finding
            });
        }
    }

    #[test]
    fn idle_gap_past_expiry_with_paid_cache_writes_is_a_finding() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("idle");
        edit_cache(&mut evidence, false, |cache| {
            cache.cache_creation_tokens = 1;
            cache.longest_idle_gap_ms = catalogs.cache_idle_expiry_ms;
        });

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn churn_event_without_paid_cache_writes_is_no_finding() {
        let mut evidence = claude_evidence("free-churn");
        edit_cache(&mut evidence, false, |cache| {
            cache.user_controlled_churn.manual_compactions = 2;
        });

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn quiet_cache_is_no_finding() {
        assert_eq!(
            evaluate(&claude_evidence("quiet"), &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn repeated_context_at_the_threshold_is_a_finding_without_a_churn_event() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("repeated");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Complete(RepeatedContext {
                accounting: RepeatedContextAccounting::CacheWrite,
                repeated_tokens: catalogs.repeated_context_tokens_threshold,
                pairs_considered: 1,
                pairs_skipped: 0,
            });
        });

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn uncached_input_accounting_below_threshold_with_a_churn_event_is_no_finding() {
        // The legacy churn-event rule reads `cache_creation_tokens`, which
        // uncached-input sources never populate: it must not fire under
        // this accounting even when a churn event and repeated tokens both
        // sit below the threshold.
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("uncached-churn");
        edit_cache(&mut evidence, false, |cache| {
            cache.cache_creation_tokens = 5_000;
            cache.model_transitions.push(ModelTransition {
                ts_ms: 10,
                from_model: "model-a".to_owned(),
                to_model: "model-b".to_owned(),
            });
            cache.repeated_context = EvidenceValue::Complete(RepeatedContext {
                accounting: RepeatedContextAccounting::UncachedInput,
                repeated_tokens: 1,
                pairs_considered: 1,
                pairs_skipped: 0,
            });
        });

        assert_eq!(evaluate(&evidence, &catalogs), Observation::NoFinding);
    }

    #[test]
    fn unsupported_repeated_context_is_a_contract_gap() {
        let mut evidence = claude_evidence("unsupported-accounting");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Unsupported;
        });

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
        );
    }
}
