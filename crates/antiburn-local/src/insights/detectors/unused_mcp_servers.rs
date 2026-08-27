// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unused MCP Servers: MCP servers loaded into eligible sessions and
//! never directly invoked.
//!
//! The finding is an absence claim about invocation, so partial
//! evidence never permits it: a partial tools or context-sources group
//! may have missed the invoking record, and a never-invoked flag from
//! it would be a false positive.
//!
//! Partial-evidence rules:
//! - No partial evidence permits a finding. The finding requires
//!   complete context-sources, tools, and eligibility groups.
//! - Partial evidence in any required group prevents clean.

use crate::analysis::SessionEvidence;

use super::{Observation, complete};

pub(crate) fn evaluate(evidence: &SessionEvidence) -> Observation {
    let (Some(sources), Some(_tools), Some(eligibility)) = (
        complete(&evidence.context_sources),
        complete(&evidence.tools),
        complete(&evidence.eligibility),
    ) else {
        return Observation::NoFinding;
    };
    if eligibility.assistant_turns == 0 {
        return Observation::NoFinding;
    }
    if sources.mcp_servers.values().any(|server| !server.invoked) {
        return Observation::Finding;
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, LoadedSource};

    fn with_server(invoked: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("mcp");
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 3;
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.mcp_servers.insert(
            "server-a".to_owned(),
            LoadedSource {
                description: None,
                invoked,
                origin: EvidenceValue::Unsupported,
            },
        );
        evidence
    }

    #[test]
    fn loaded_and_never_invoked_server_is_a_finding() {
        assert_eq!(evaluate(&with_server(false)), Observation::Finding);
    }

    #[test]
    fn invoked_server_is_no_finding() {
        assert_eq!(evaluate(&with_server(true)), Observation::NoFinding);
    }

    #[test]
    fn partial_tools_coverage_never_claims_the_absence_finding() {
        let mut evidence = with_server(false);
        evidence.tools = match evidence.tools {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::MalformedRecord,
            },
            _ => unreachable!(),
        };

        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }

    #[test]
    fn session_without_assistant_work_is_no_finding() {
        let mut evidence = with_server(false);
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 0;

        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }
}
