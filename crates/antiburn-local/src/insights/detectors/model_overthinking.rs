//! Model Overthinking: turns that use an explicit reasoning or effort
//! tier above the report-time recommended cap.
//!
//! Only explicitly observed tiers count. The parser never infers a
//! tier from prompt keywords, so every counted tier is direct evidence.
//! Tier policy is keyed by model family (`ReportCatalogs::families`):
//! Claude and OpenAI classify overlapping labels (for example `high`)
//! differently, so the same string is not the same tier across
//! harnesses. The families present in a session come from its observed
//! `models.by_model` keys, normalized the same way.
//!
//! Partial-evidence rules:
//! - Partial model evidence still permits a finding. An observed
//!   above-cap tier with turns proves presence.
//! - Partial model evidence prevents clean. A missed record may hide
//!   an above-cap tier, so absence cannot be concluded.
//! - A tier with turns that no present family recognizes (including a
//!   session where the only present family is `Unknown`) blocks clean
//!   with a contract gap: the policy has not classified it, so absence
//!   is never provable.
//! - A finding still wins when no turn reports an effort value.
//!   Otherwise the rule assesses only the turns that do report a
//!   value. A turn without the signal is not negative evidence.
//!   Claude Code writes the effort field on every main-loop turn. It
//!   omits the effort field on most delegated turns. A full-coverage
//!   rule never clears a session with subagent work. Cadence, the
//!   golden source, applies the same principle to its `speed` field in
//!   `crates/analysis/src/efficiency_findings.rs`: the field's doc
//!   states that absence is never negative evidence of "not fast",
//!   and its `detect_fast_mode` function skips a no-signal turn
//!   instead of failing the whole session. The rule reports the
//!   signal as missing only when no turn in the session carries an
//!   effort value.

use std::collections::BTreeSet;

use crate::analysis::SessionEvidence;

use super::{ModelFamily, Observation, ReportCatalogs, model_family, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    if let Some(models) = observed(&evidence.models) {
        let present_families: BTreeSet<ModelFamily> = models
            .by_model
            .keys()
            .map(|model| model_family(model))
            .collect();

        let mut contract_incomplete = false;
        for (tier, turns) in &models.effort_tiers {
            if turns.main_loop + turns.delegated == 0 {
                continue;
            }
            let normalized = tier.trim().to_lowercase();
            let above_cap = present_families.iter().any(|family| {
                catalogs
                    .families
                    .get(family)
                    .is_some_and(|policy| policy.effort.above_cap.contains(&normalized))
            });
            if above_cap {
                return Observation::Finding;
            }
            let recognized = present_families.iter().any(|family| {
                !matches!(family, ModelFamily::Unknown)
                    && catalogs
                        .families
                        .get(family)
                        .is_some_and(|policy| policy.effort.recognized.contains(&normalized))
            });
            if !recognized {
                contract_incomplete = true;
            }
        }
        if contract_incomplete {
            return Observation::ContractIncomplete;
        }
        // A turn without an effort value is not negative evidence. The
        // rule assesses only the turns that report one. The rule
        // reports the signal as missing only when no turn reports one.
        // This rule also covers zero eligible turns, because
        // `present_turns` is then zero too.
        let coverage = models.effort_signal;
        if coverage.present_turns == 0 {
            return Observation::SignalMissing;
        }
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, ModelTokens, SignalCoverage, TurnCounts};

    /// Builds evidence with one effort-tier entry, one Claude `by_model`
    /// entry (so the Claude family is present), and full effort-signal
    /// coverage: every eligible turn carried an effort value.
    fn with_tier(tier: &str, partial: bool) -> SessionEvidence {
        with_tier_and_model(tier, "claude-sonnet-4-6", partial)
    }

    fn with_tier_and_model(tier: &str, model: &str, partial: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("effort");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.effort_tiers.insert(
            tier.to_owned(),
            TurnCounts {
                main_loop: 1,
                delegated: 0,
            },
        );
        models
            .by_model
            .insert(model.to_owned(), ModelTokens::default());
        models.effort_signal = SignalCoverage {
            eligible_turns: 1,
            present_turns: 1,
        };
        evidence.models = if partial {
            EvidenceValue::Partial {
                observed: models,
                reason: CoverageReason::MalformedRecord,
            }
        } else {
            EvidenceValue::Complete(models)
        };
        evidence
    }

    #[test]
    fn above_cap_tier_is_a_finding_even_from_partial_evidence() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_tier("max", false), &catalogs),
            Observation::Finding
        );
        assert_eq!(
            evaluate(&with_tier("max", true), &catalogs),
            Observation::Finding
        );
    }

    #[test]
    fn tier_within_the_cap_is_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_tier("medium", false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn no_effort_signal_present_reports_the_signal_as_missing() {
        // Zero eligible turns leaves `present_turns` at zero too. This
        // also proves the eligible_turns == 0 case stays missing.
        let catalogs = ReportCatalogs::default();
        let evidence = claude_evidence("no-eligible-turns");

        assert_eq!(evaluate(&evidence, &catalogs), Observation::SignalMissing);
    }

    #[test]
    fn partial_effort_signal_coverage_without_a_finding_is_no_finding() {
        // At least one turn carries an effort value. The rule assesses
        // that turn instead of reporting the signal as missing. A turn
        // without the signal is not negative evidence.
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("partial-effort-coverage");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.effort_signal = SignalCoverage {
            eligible_turns: 3,
            present_turns: 1,
        };
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::NoFinding);
    }

    #[test]
    fn full_effort_signal_coverage_without_a_finding_is_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_tier("medium", false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_finding_wins_over_partial_effort_signal_coverage() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = with_tier("max", false);
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.effort_signal = SignalCoverage {
            eligible_turns: 3,
            present_turns: 1,
        };
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn xhigh_and_max_are_above_cap_for_both_families() {
        let catalogs = ReportCatalogs::default();

        for (tier, model) in [
            ("xhigh", "gpt-5.6"),
            ("xhigh", "claude-sonnet-4-6"),
            ("max", "gpt-5.6"),
            ("max", "claude-sonnet-4-6"),
        ] {
            assert_eq!(
                evaluate(&with_tier_and_model(tier, model, false), &catalogs),
                Observation::Finding,
                "{tier} on {model}"
            );
        }
    }

    #[test]
    fn a_family_floor_tier_is_recognized_only_by_that_family() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_tier_and_model("minimal", "gpt-5.6", false), &catalogs),
            Observation::NoFinding
        );
        // Claude never emits `minimal`, so the only present family
        // cannot classify it and it blocks clean.
        assert_eq!(
            evaluate(
                &with_tier_and_model("minimal", "claude-sonnet-4-6", false),
                &catalogs
            ),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn an_unrecognized_tier_with_a_known_family_present_is_contract_incomplete() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(
                &with_tier_and_model("synthetic-tier", "claude-sonnet-4-6", false),
                &catalogs
            ),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn a_tier_with_no_present_family_is_contract_incomplete() {
        // No `by_model` entry means no family is present at all, so
        // even a normally-recognized label cannot be classified.
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("no-model-observed");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.effort_tiers.insert(
            "medium".to_owned(),
            TurnCounts {
                main_loop: 1,
                delegated: 0,
            },
        );
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(
            evaluate(&evidence, &catalogs),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn label_normalization_trims_and_lowercases_before_policy_lookup() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_tier("  Max  ", false), &catalogs),
            Observation::Finding
        );
    }
}
