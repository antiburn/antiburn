// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unused Built-In Tools: native harness tool definitions that consume
//! context and are never used.
//!
//! The current evidence contract carries no definition names:
//! `ContextSourceEvidence::tool_definitions` is a bare marker. Without
//! names, no unused definition can be identified and no absence can be
//! concluded. Every eligible session therefore reports
//! `ContractIncomplete` — never a finding and never clean — until
//! CH-009 joins built-in definitions to invocation evidence.
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
