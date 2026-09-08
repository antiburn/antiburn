//! Unused MCP Servers: MCP servers loaded into eligible sessions and
//! never directly invoked.
//!
//! The finding requires an observed injection and complete invocation coverage.
//!
//! Findings require complete coverage of observed MCP resources, tools, and eligibility.
//! An unrelated partial resource group does not block a finding.
//! Observed resources do not prove a full historical inventory, so the report cannot claim clean.

use crate::analysis::{EvidenceValue, SessionEvidence};
use crate::remediation::FindingCause;

use super::{Observation, complete};

pub(crate) fn evaluate(evidence: &SessionEvidence) -> Observation {
    let (Some(sources), Some(_tools), Some(eligibility)) = (
        match &evidence.context_sources {
            EvidenceValue::Complete(sources)
            | EvidenceValue::Partial {
                observed: sources, ..
            } => Some(sources),
            EvidenceValue::Unsupported => None,
        },
        complete(&evidence.tools),
        complete(&evidence.eligibility),
    ) else {
        return Observation::NoFinding;
    };
    if eligibility.assistant_turns == 0 {
        return Observation::NoFinding;
    }
    match sources.mcp_coverage {
        EvidenceValue::Unsupported => return Observation::SignalMissing,
        EvidenceValue::Partial { .. } => return Observation::NoFinding,
        EvidenceValue::Complete(()) => {}
    }
    if sources
        .mcp_servers
        .values()
        .any(|server| server.injected && !server.invoked)
    {
        return Observation::Finding;
    }
    Observation::NoFinding
}

pub(super) fn finding_causes(evidence: &SessionEvidence) -> Vec<FindingCause> {
    let Some(sources) = super::observed(&evidence.context_sources) else {
        return Vec::new();
    };
    sources
        .mcp_servers
        .iter()
        .filter(|(_, server)| server.injected && !server.invoked)
        .map(|(server, _)| FindingCause::UnusedMcpServer {
            server: server.clone(),
        })
        .collect()
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
                configured: true,
                available: true,
                injected: true,
                invoked,
                token_count: None,
                origin: EvidenceValue::Unsupported,
            },
        );
        sources.mcp_coverage = EvidenceValue::Complete(());
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
    fn available_server_without_injection_is_no_finding() {
        let mut evidence = with_server(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.mcp_servers.get_mut("server-a").unwrap().injected = false;
        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }

    #[test]
    fn missing_mcp_coverage_never_claims_non_use() {
        let mut evidence = with_server(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.mcp_coverage = EvidenceValue::Unsupported;
        assert_eq!(evaluate(&evidence), Observation::SignalMissing);
    }

    #[test]
    fn incomplete_skill_attribution_does_not_block_mcp() {
        let mut evidence = with_server(false);
        let EvidenceValue::Complete(mut sources) = evidence.context_sources else {
            unreachable!()
        };
        sources.skill_coverage = EvidenceValue::Partial {
            observed: (),
            reason: CoverageReason::AttributionIncomplete,
        };
        evidence.context_sources = EvidenceValue::Partial {
            observed: sources,
            reason: CoverageReason::AttributionIncomplete,
        };
        assert_eq!(evaluate(&evidence), Observation::Finding);
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
