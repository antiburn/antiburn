//! Overpowered Subagents: premium main-loop models that spawn
//! subagents on the same premium tier.
//!
//! The current evidence contract carries no child model identity:
//! `SubagentChild::child_model` is a bare marker. A spawned child can
//! therefore not be classified against the parent tier. A session with
//! observed spawns — or with delegated turns, which the sink counts
//! independently of spawn records — reports `ContractIncomplete`,
//! never a finding and never clean, until CH-009 carries child model
//! identity.
//!
//! Partial-evidence rules:
//! - A complete-zero spawn count proves absence and supports clean.
//! - Partial subagent evidence with zero observed spawns prevents
//!   clean. A missed record may hide a spawn.
//! - Once child model identity ships, an observed premium child from
//!   partial evidence will permit a finding.

use crate::analysis::SessionEvidence;

use super::{Observation, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence) -> Observation {
    if let Some(subagents) = observed(&evidence.subagents)
        && (subagents.spawn_count > 0
            || subagents.delegated_turns > 0
            || !subagents.children.is_empty())
    {
        return Observation::ContractIncomplete;
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::EvidenceValue;

    #[test]
    fn zero_spawns_report_no_finding() {
        assert_eq!(
            evaluate(&claude_evidence("no-spawns")),
            Observation::NoFinding
        );
    }

    #[test]
    fn observed_spawns_report_the_contract_gap_instead_of_a_verdict() {
        let mut evidence = claude_evidence("spawns");
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            unreachable!()
        };
        subagents.spawn_count = 1;

        assert_eq!(evaluate(&evidence), Observation::ContractIncomplete);
    }

    #[test]
    fn delegated_turns_without_spawn_records_report_the_contract_gap() {
        let mut evidence = claude_evidence("delegated-only");
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            unreachable!()
        };
        subagents.spawn_count = 0;
        subagents.delegated_turns = 1;

        assert_eq!(evaluate(&evidence), Observation::ContractIncomplete);
    }
}
