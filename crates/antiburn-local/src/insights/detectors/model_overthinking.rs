// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Model Overthinking: turns that use an explicit reasoning or effort
//! tier above the report-time recommended cap.
//!
//! Only explicitly observed tiers count. The parser never infers a
//! tier from prompt keywords, so every counted tier is direct evidence.
//!
//! Partial-evidence rules:
//! - Partial model evidence still permits a finding. An observed
//!   above-cap tier with turns proves presence.
//! - Partial model evidence prevents clean. A missed record may hide
//!   an above-cap tier, so absence cannot be concluded.

use crate::analysis::SessionEvidence;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    if let Some(models) = observed(&evidence.models) {
        for (tier, turns) in &models.effort_tiers {
            if catalogs.effort_tiers_above_cap.contains(tier)
                && turns.main_loop + turns.delegated > 0
            {
                return Observation::Finding;
            }
        }
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, TurnCounts};

    fn with_tier(tier: &str, partial: bool) -> SessionEvidence {
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
        let tier = catalogs.effort_tiers_above_cap.first().unwrap().clone();

        assert_eq!(
            evaluate(&with_tier(&tier, false), &catalogs),
            Observation::Finding
        );
        assert_eq!(
            evaluate(&with_tier(&tier, true), &catalogs),
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
}
