//! Overuse of Fast Mode: explicit fast-tier usage in delegated work.
//!
//! The evidence separates main-loop from delegated fast-tier turns.
//! A standing-default signal does not exist in the contract yet, so
//! the rule covers only the delegated-work half of the category.
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

use crate::analysis::SessionEvidence;

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
        let delegated: u64 = models
            .fast_modes
            .values()
            .map(|turns| turns.delegated)
            .sum();
        if delegated > 0 && delegated >= catalogs.fast_mode_delegated_turns_threshold {
            return Observation::Finding;
        }
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, TurnCounts};

    fn with_fast_turns(main_loop: u64, delegated: u64, partial: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("fast");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.fast_modes.insert(
            "fast".to_owned(),
            TurnCounts {
                main_loop,
                delegated,
            },
        );
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
}
