//! Unused Built-In Tools: native harness tool definitions that consume
//! context and are never used.
//!
//! Stored detector evidence carries no definition names, so this pure
//! evaluator cannot identify an unused definition. The report reducer can
//! assess this check when the separate initial-context projection supplies
//! complete named definitions and invocation counts.
//!
//! Partial-evidence rules:
//! - No partial evidence permits a finding: the unused claim is an
//!   absence claim and needs complete named definitions.
//! - No evidence state produces clean while the payload is missing.

use crate::analysis::SessionEvidence;

use super::Observation;

pub(crate) fn evaluate(_evidence: &SessionEvidence) -> Observation {
    Observation::ContractIncomplete
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;

    #[test]
    fn the_marker_payload_reports_the_contract_gap() {
        assert_eq!(
            evaluate(&claude_evidence("built-in")),
            Observation::ContractIncomplete
        );
    }
}
