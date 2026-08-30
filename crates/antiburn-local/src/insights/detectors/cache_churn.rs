//! Cache Churn: per-thread repeated-context accounting scored as an
//! overpay multiple, matching Cadence's `detect_cache_rehydration`
//! (`crates/analysis/src/efficiency_findings.rs`).
//!
//! Cadence has no absolute token threshold. It computes
//! `paid_per_unique_token = total_paid / unique_paid_tokens` where
//! `unique_paid_tokens = total_paid - overpaid`, and calls a finding
//! when that multiple reaches the family's reviewed "avg efficiency"
//! band bound. This rule ports that math onto
//! `RepeatedContext::{paid_tokens, repeated_tokens}` (`repeated_tokens`
//! is antiburn's name for Cadence's `overpaid`), summed per session
//! instead of per 30-day user.
//!
//! The bound comes from `catalogs.families[family]
//! .cache_overpay_multiple_threshold`, where `family` is
//! `ModelEvidence::dominant_main_model`'s family, falling back to the
//! family of the first model key in `by_model` (`BTreeMap` order) when
//! no dominant model was computed. Neither present, or the resolved
//! family's premium policy is not reviewed (including
//! `ModelFamily::Unknown`, which is never reviewed), reports a
//! contract gap: the rule reuses the premium policy's `reviewed` flag
//! rather than adding a second per-family flag, since both flags mean
//! the same thing — a maintainer has classified this family's models.
//!
//! The finding fires only when `repeated_tokens > 0` (Cadence: "only
//! ever fires when overpaid > 0") and the multiple reaches the bound.
//! `unique_paid_tokens == 0` (every paid token in a considered pair was
//! a repeat) is treated as an infinite multiple, so it is always a
//! finding.
//!
//! Partial-evidence rules:
//! - Partial cache evidence still permits a finding: repeated and paid
//!   tokens read from partial evidence prove presence.
//! - Partial cache evidence prevents clean. A missed record may hide
//!   repeated context or understate the paid total.
//! - `repeated_context` `Unsupported` reports a contract gap: the
//!   source supports neither cache-write nor uncached-input
//!   accounting.

use crate::analysis::{EvidenceValue, SessionEvidence};

use super::{ModelFamily, Observation, ReportCatalogs, model_family, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    let Some(cache) = observed(&evidence.cache) else {
        return Observation::ContractIncomplete;
    };
    let repeated_context = match &cache.repeated_context {
        EvidenceValue::Unsupported => return Observation::ContractIncomplete,
        EvidenceValue::Partial { observed, .. } => observed,
        EvidenceValue::Complete(observed) => observed,
    };
    if repeated_context.repeated_tokens == 0 {
        return Observation::NoFinding;
    }

    let Some(family) = dominant_family(evidence) else {
        return Observation::ContractIncomplete;
    };
    let Some(policy) = catalogs.families.get(&family) else {
        return Observation::ContractIncomplete;
    };
    if !policy.premium.reviewed {
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

/// Resolves the model family the overpay bound is judged under:
/// `ModelEvidence::dominant_main_model`'s family, or, when that is
/// absent, the family of the first model key in `by_model` (`BTreeMap`
/// order, so this is deterministic). `None` when neither is present.
fn dominant_family(evidence: &SessionEvidence) -> Option<ModelFamily> {
    let models = observed(&evidence.models)?;
    if let Some(dominant) = models.dominant_main_model.as_deref() {
        return Some(model_family(dominant));
    }
    let first_model = models.by_model.keys().next()?;
    Some(model_family(first_model))
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

            assert_eq!(evaluate(&evidence, &ReportCatalogs::default()), {
                Observation::Finding
            });
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
    fn unknown_family_is_a_contract_gap_even_above_the_claude_bound() {
        let mut evidence = claude_evidence("unknown-family");
        edit_cache(&mut evidence, false, |cache| {
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                300,
                400,
            ));
        });
        set_dominant_main_model(&mut evidence, "some-unlisted-model");

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn no_dominant_model_falls_back_to_the_first_by_model_key() {
        let mut evidence = claude_evidence("fallback-family");
        edit_cache(&mut evidence, false, |cache| {
            // multiple = 400 / (400 - 300) = 4.0, above the Claude bound.
            cache.repeated_context = EvidenceValue::Complete(repeated_context(
                RepeatedContextAccounting::CacheWrite,
                300,
                400,
            ));
        });
        // `dominant_main_model` stays `None` (the fixture's default);
        // only `by_model` carries a key, so the fallback must resolve
        // the family from it.
        let EvidenceValue::Complete(mut models) = evidence.models.clone() else {
            unreachable!()
        };
        models
            .by_model
            .insert("claude-sonnet-4-6".to_owned(), ModelTokens::default());
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn no_present_family_at_all_is_a_contract_gap() {
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
            Observation::ContractIncomplete
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
