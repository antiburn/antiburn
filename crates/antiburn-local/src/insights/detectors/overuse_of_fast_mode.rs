//! Overuse of Fast Mode: explicit fast-tier usage in delegated work.
//!
//! The evidence separates main-loop from delegated fast-tier turns.
//! A standing-default signal does not exist in the contract yet, so
//! the rule covers only the delegated-work half of the category.
//! Both shipped families recognize exactly the normalized labels
//! `fast` and `standard` (`ReportCatalogs::families`), so — unlike
//! Model Overthinking — the rule does not need to know which family
//! produced a turn to classify its speed label.
//!
//! Partial-evidence rules:
//! - Partial model evidence still permits a finding. Observed
//!   delegated fast-tier turns prove presence.
//! - Partial model evidence prevents clean. A missed record may hide
//!   delegated fast-tier work.
//! - A source without the fast-tier capability reports the contract
//!   gap: the eligibility clause admits a session on service-tier
//!   alone, but `ModelEvidence::service_tiers` is a bare marker the
//!   rule cannot read, so neither a finding nor clean is expressible.
//! - The rule counts only the normalized `fast` label. Any other
//!   recognized label (for example `"standard"`) never counts toward
//!   the finding.
//! - A `fast_modes` key with turns that normalizes to neither `fast`
//!   nor `standard` blocks clean with a contract gap: the policy has
//!   not classified the label, so absence is never provable. A finding
//!   still wins over this gap.
//! - A finding still wins when no turn reports a speed value.
//!   Otherwise the rule assesses only the turns that do report a
//!   value. A turn without the signal is not negative evidence.
//!   Claude Code writes `speed` on every main-loop turn. It omits
//!   `speed` on most delegated turns. A full-coverage rule never
//!   clears a session with subagent work. Cadence, the golden source,
//!   applies the same principle in
//!   `crates/analysis/src/efficiency_findings.rs`: its turn `speed`
//!   field doc states that absence is never negative evidence of "not
//!   fast", and its `detect_fast_mode` function skips a no-signal turn
//!   instead of failing the whole session. The rule reports the
//!   signal as missing only when no turn in the session carries a
//!   speed value.

use crate::analysis::{FAST_SPEED_KEY, SessionEvidence};

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    // The eligibility clause accepts service-tier as an alternative to
    // fast-tier, but the rule reads only `models.fast_modes`;
    // `ModelEvidence::service_tiers` is a bare marker it cannot read.
    // Mirror the Overpowered Subagents guard: report the contract gap
    // instead of a verdict the evidence cannot support.
    if !evidence.capabilities.fast_tier {
        return Observation::ContractIncomplete;
    }
    if let Some(models) = observed(&evidence.models) {
        let mut contract_incomplete = false;
        for (label, turns) in &models.fast_modes {
            let turns_with_signal = turns.main_loop + turns.delegated;
            if turns_with_signal == 0 {
                continue;
            }
            let normalized = label.trim().to_lowercase();
            if normalized == FAST_SPEED_KEY {
                if turns.delegated > 0
                    && turns.delegated >= catalogs.fast_mode_delegated_turns_threshold
                {
                    return Observation::Finding;
                }
            } else if !is_recognized_speed(&normalized, catalogs) {
                contract_incomplete = true;
            }
        }
        if contract_incomplete {
            return Observation::ContractIncomplete;
        }
        // A turn without a speed value is not negative evidence. The
        // rule assesses only the turns that report one. The rule
        // reports the signal as missing only when no turn reports one.
        // This rule also covers zero eligible turns, because
        // `present_turns` is then zero too.
        let coverage = models.speed_signal;
        if coverage.present_turns == 0 {
            return Observation::SignalMissing;
        }
    }
    Observation::NoFinding
}

/// Returns whether any family's policy recognizes `label`, which is
/// already trimmed and lowercased. Both shipped families recognize the
/// same speed vocabulary, so this does not need per-session family
/// derivation the way Model Overthinking's effort check does.
fn is_recognized_speed(label: &str, catalogs: &ReportCatalogs) -> bool {
    catalogs
        .families
        .values()
        .any(|policy| policy.speed.recognized.contains(label))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, SignalCoverage, TurnCounts};

    /// Builds evidence with one `FAST_SPEED_KEY` entry and full speed-
    /// signal coverage: every eligible turn carried a speed value.
    fn with_fast_turns(main_loop: u64, delegated: u64, partial: bool) -> SessionEvidence {
        with_speed_entry(FAST_SPEED_KEY, main_loop, delegated, partial)
    }

    fn with_speed_entry(
        label: &str,
        main_loop: u64,
        delegated: u64,
        partial: bool,
    ) -> SessionEvidence {
        let mut evidence = claude_evidence("fast");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.fast_modes.insert(
            label.to_owned(),
            TurnCounts {
                main_loop,
                delegated,
            },
        );
        let turns = main_loop + delegated;
        models.speed_signal = SignalCoverage {
            eligible_turns: turns,
            present_turns: turns,
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
    fn delegated_fast_turns_are_a_finding_even_from_partial_evidence() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_fast_turns(0, 1, false), &catalogs),
            Observation::Finding
        );
        assert_eq!(
            evaluate(&with_fast_turns(0, 1, true), &catalogs),
            Observation::Finding
        );
    }

    #[test]
    fn main_loop_fast_turns_alone_are_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_fast_turns(5, 0, false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_delegated_standard_turn_never_counts_as_fast_mode_overuse() {
        // A delegated turn on the "standard" speed label must never
        // produce a finding: only the FAST_SPEED_KEY entry counts.
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_speed_entry("standard", 0, 1, false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_service_tier_only_source_reports_the_contract_gap() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("service-tier-only");
        evidence.capabilities.fast_tier = false;
        evidence.capabilities.service_tier = true;

        assert_eq!(
            evaluate(&evidence, &catalogs),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn no_speed_signal_present_reports_the_signal_as_missing() {
        // Zero eligible turns leaves `present_turns` at zero too. This
        // also proves the eligible_turns == 0 case stays missing.
        let catalogs = ReportCatalogs::default();
        let evidence = claude_evidence("no-eligible-turns");

        assert_eq!(evaluate(&evidence, &catalogs), Observation::SignalMissing);
    }

    #[test]
    fn partial_speed_signal_coverage_without_a_finding_is_no_finding() {
        // At least one turn carries a speed value. The rule assesses
        // that turn instead of reporting the signal as missing. A turn
        // without the signal is not negative evidence.
        let catalogs = ReportCatalogs::default();
        let mut evidence = claude_evidence("partial-speed-coverage");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.speed_signal = SignalCoverage {
            eligible_turns: 3,
            present_turns: 1,
        };
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::NoFinding);
    }

    #[test]
    fn full_speed_signal_coverage_without_a_finding_is_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_fast_turns(5, 0, false), &catalogs),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_finding_wins_over_partial_speed_signal_coverage() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = with_fast_turns(0, 1, false);
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.speed_signal = SignalCoverage {
            eligible_turns: 3,
            present_turns: 1,
        };
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn an_unrecognized_speed_label_with_turns_is_contract_incomplete() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_speed_entry("synthetic-tier", 0, 1, false), &catalogs),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn a_finding_wins_over_an_unrecognized_speed_label_elsewhere_in_the_session() {
        let catalogs = ReportCatalogs::default();
        let mut evidence = with_fast_turns(0, 1, false);
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.fast_modes.insert(
            "synthetic-tier".to_owned(),
            TurnCounts {
                main_loop: 0,
                delegated: 1,
            },
        );
        evidence.models = EvidenceValue::Complete(models);

        assert_eq!(evaluate(&evidence, &catalogs), Observation::Finding);
    }

    #[test]
    fn label_normalization_trims_and_lowercases_before_policy_lookup() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_speed_entry("  Fast  ", 0, 1, false), &catalogs),
            Observation::Finding
        );
    }
}
