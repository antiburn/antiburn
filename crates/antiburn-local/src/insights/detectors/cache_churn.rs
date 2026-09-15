//! Cache Churn: per-thread repeated-context accounting scored as an
//! overpay multiple.
//!
//! The rule has no absolute token threshold. It computes
//! `paid_per_unique_token = total_paid / unique_paid_tokens` where
//! `unique_paid_tokens = paid_tokens - repeated_tokens`. It reports a
//! finding when that multiple reaches the family's reviewed average
//! efficiency bound. The token totals are summed per session rather
//! than per user over 30 days.
//!
//! The bound comes from the reviewed family for the repeated-context
//! accounting contract. Cache-write accounting uses the Claude policy;
//! uncached-input accounting uses the OpenAI policy.
//!
//! The finding fires only when `repeated_tokens > 0` and the multiple
//! reaches the bound.
//! `unique_paid_tokens == 0` (every eligible paid token was
//! a repeat) is treated as an infinite multiple, so it is always a
//! finding.
//!
//! Partial evidence cannot establish the session ratio or a clean result.
//! Missing payments can change the denominator and the threshold outcome.

use crate::analysis::{EvidenceValue, SessionEvidence};
use crate::remediation::FindingCause;

use super::{ModelFamily, Observation, ReportCatalogs, model_family, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    let cache = match &evidence.cache {
        EvidenceValue::Complete(cache) => cache,
        EvidenceValue::Partial { .. } => return Observation::NoFinding,
        EvidenceValue::Unsupported => return Observation::ContractIncomplete,
    };
    let repeated_context = match &cache.repeated_context {
        EvidenceValue::Partial { .. } => return Observation::NoFinding,
        EvidenceValue::Unsupported => return Observation::ContractIncomplete,
        EvidenceValue::Complete(observed) => observed,
    };
    if repeated_context.repeated_tokens == 0 {
        return Observation::NoFinding;
    }

    let family = accounting_family(repeated_context.accounting);
    let Some(policy) = catalogs.families.get(&family) else {
        return Observation::ContractIncomplete;
    };
    if !policy.cache_policy_reviewed {
        return Observation::ContractIncomplete;
    }

    let unique_paid_tokens = repeated_context
        .paid_tokens
        .saturating_sub(repeated_context.repeated_tokens);
    if unique_paid_tokens == 0 {
        return Observation::Finding;
    }
    let multiple = repeated_context.paid_tokens as f64 / unique_paid_tokens as f64;
    if multiple >= policy.cache_overpay_multiple_threshold {
        Observation::Finding
    } else {
        Observation::NoFinding
    }
}

pub(super) fn finding_causes(
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Vec<FindingCause> {
    let Some(cache) = observed(&evidence.cache) else {
        return Vec::new();
    };
    let Some(repeated) = observed(&cache.repeated_context) else {
        return Vec::new();
    };
    let family = accounting_family(repeated.accounting);
    let Some(policy) = catalogs.families.get(&family) else {
        return Vec::new();
    };
    let Some(model) = observed(&evidence.models).and_then(|models| {
        models
            .dominant_main_model
            .as_ref()
            .filter(|model| model_family(model) == family)
            .or_else(|| {
                models
                    .by_model
                    .keys()
                    .find(|model| model_family(model) == family)
            })
    }) else {
        return Vec::new();
    };
    vec![FindingCause::CacheChurn {
        model: model.clone(),
        repeated_tokens: repeated.repeated_tokens,
        paid_tokens: repeated.paid_tokens,
        threshold_basis_points: (policy.cache_overpay_multiple_threshold * 10_000.0).round() as u32,
    }]
}

fn accounting_family(accounting: crate::analysis::RepeatedContextAccounting) -> ModelFamily {
    match accounting {
        crate::analysis::RepeatedContextAccounting::CacheWrite => ModelFamily::Claude,
        crate::analysis::RepeatedContextAccounting::UncachedInput => ModelFamily::OpenAi,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{
        CacheEvidence, CoverageReason, EvidenceValue, ModelTokens, RepeatedContextAccounting,
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

    fn repeated_context(
        accounting: RepeatedContextAccounting,
        repeated_tokens: u64,
        paid_tokens: u64,
    ) -> crate::analysis::RepeatedContext {
        crate::analysis::RepeatedContext {
            accounting,
            repeated_tokens,
            paid_tokens,
            pairs_considered: 1,
            pairs_skipped: 0,
        }
    }

    #[test]
    fn claude_at_the_bound_is_a_finding() {
        for partial in [false, true] {
            let mut evidence = claude_evidence("claude-at-bound");
            edit_cache(&mut evidence, partial, |cache| {
                // multiple = 235 / (235 - 135) = 2.35, exactly the bound.
                cache.repeated_context = EvidenceValue::Complete(repeated_context(
                    RepeatedContextAccounting::CacheWrite,
                    135,
                    235,
                ));
            });
            set_dominant_main_model(&mut evidence, "claude-sonnet-4-6");

            assert_eq!(
                evaluate(&evidence, &ReportCatalogs::default()),
                if partial {
                    Observation::NoFinding
                } else {
                    Observation::Finding
                }
            );
        }
    }

    #[test]
    fn claude_above_the_bound_is_a_finding() {
        let mut evidence = claude_evidence("claude-above-bound");
        edit_cache(&mut evidence, false, |cache| {
            // multiple = 400 / (400 - 300) = 4.0, above 2.35.
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                300,
                400,
            ));
        });
        set_dominant_main_model(&mut evidence, "claude-sonnet-4-6");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn claude_below_the_bound_is_no_finding() {
        let mut evidence = claude_evidence("claude-below-bound");
        edit_cache(&mut evidence, false, |cache| {
            // multiple = 200 / (200 - 50) = 1.333..., below 2.35.
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                50,
                200,
            ));
        });
        set_dominant_main_model(&mut evidence, "claude-sonnet-4-6");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn openai_at_the_bound_is_a_finding() {
        let mut evidence = claude_evidence("openai-at-bound");
        edit_cache(&mut evidence, false, |cache| {
            // multiple = 200 / (200 - 100) = 2.0, exactly the OpenAI bound.
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::UncachedInput,
                100,
                200,
            ));
        });
        set_dominant_main_model(&mut evidence, "gpt-5.6-sol");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn openai_below_the_bound_is_no_finding() {
        let mut evidence = claude_evidence("openai-below-bound");
        edit_cache(&mut evidence, false, |cache| {
            // multiple = 150 / (150 - 50) = 1.5, below 2.0.
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::UncachedInput,
                50,
                150,
            ));
        });
        set_dominant_main_model(&mut evidence, "gpt-5.6-sol");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn zero_overpaid_is_no_finding_whatever_the_paid_total() {
        let mut evidence = claude_evidence("zero-overpaid");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                0,
                50_000,
            ));
        });
        set_dominant_main_model(&mut evidence, "claude-sonnet-4-6");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn every_paid_token_repeated_is_a_finding_via_the_infinite_multiple_guard() {
        let mut evidence = claude_evidence("all-repeated");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                80,
                80,
            ));
        });
        set_dominant_main_model(&mut evidence, "claude-sonnet-4-6");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn cache_write_accounting_uses_the_claude_policy_in_a_mixed_family_session() {
        let mut evidence = claude_evidence("mixed-cache-write-family");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                120,
                220,
            ));
        });
        set_dominant_main_model(&mut evidence, "gpt-5.6-sol");
        let EvidenceValue::Complete(mut models) = evidence.models.clone() else {
            unreachable!()
        };
        models
            .by_model
            .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn uncached_input_accounting_uses_openai_policy_in_a_mixed_family_session() {
        let mut evidence = claude_evidence("mixed-family");
        edit_cache(&mut evidence, false, |cache| {
            // The 2.1 multiple exceeds the OpenAI bound but not the Claude bound.
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::UncachedInput,
                110,
                210,
            ));
        });
        set_dominant_main_model(&mut evidence, "claude-sonnet-4-6");
        let EvidenceValue::Complete(mut models) = evidence.models.clone() else {
            unreachable!()
        };
        models
            .by_model
            .insert("gpt-5.6-sol".to_owned(), ModelTokens::default());
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
        assert!(matches!(
            finding_causes(&evidence, &ReportCatalogs::default()).as_slice(),
            [FindingCause::CacheChurn { model, .. }] if model == "gpt-5.6-sol"
        ));
    }

    #[test]
    fn policy_selection_does_not_require_model_evidence() {
        let mut evidence = claude_evidence("no-family");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                300,
                400,
            ));
        });
        evidence.models = EvidenceValue::Unsupported;

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn partial_repeated_context_cannot_establish_a_ratio() {
        let mut evidence = claude_evidence("partial-ratio");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Partial {
                observed: repeated_context(RepeatedContextAccounting::CacheWrite, 100, 100),
                reason: CoverageReason::MalformedRecord,
            };
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

    /// Overwrites `evidence.models.dominant_main_model` (and its
    /// matching `by_model` entry), keeping the rest of the fixture's
    /// model evidence.
    fn set_dominant_main_model(evidence: &mut SessionEvidence, model: &str) {
        let EvidenceValue::Complete(mut models) = evidence.models.clone() else {
            unreachable!()
        };
        models.dominant_main_model = Some(model.to_owned());
        models
            .by_model
            .insert(model.to_owned(), ModelTokens::default());
        evidence.models = EvidenceValue::Complete(models);
    }
}
