//! Unused MCP Servers: MCP servers loaded into eligible sessions and
//! never directly invoked.
//!
//! The finding requires an observed injection and complete invocation coverage.
//!
//! Findings require complete coverage of observed MCP resources, tools, and eligibility.
//! An unrelated partial resource group does not block a finding.
//! Observed resources do not prove a full historical inventory, so the report cannot claim clean.

use crate::analysis::{EvidenceCoverage, EvidenceValue, SessionEvidence};
use crate::insights::SessionTokenBurnEvidence;
use crate::insights::report::{Fact, FactState};
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
            tokens: None,
            cost_usd: None,
            pricing_revision: None,
        })
        .collect()
}

/// True when this session's flat MCP inventory cannot resolve, but the
/// report's per-turn source attribution still measured a replicated
/// definition. Mirrors `unused_built_in_tools::source_assessable`.
pub(super) fn source_assessable(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> bool {
    source_evidence
        .and_then(|value| value.mcp_sources.as_ref())
        .is_some()
        && Fact::McpInventory.state(evidence) == FactState::Unsupported
        && matches!(evidence.coverage, EvidenceCoverage::Complete)
        && matches!(&evidence.tools, EvidenceValue::Complete(_))
        && complete(&evidence.eligibility).is_some_and(|value| value.assistant_turns > 0)
}

pub(super) fn evaluate_with_source_evidence(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> Observation {
    if !source_assessable(evidence, source_evidence) {
        return evaluate(evidence);
    }
    if unused_sources(source_evidence).next().is_some() {
        Observation::Finding
    } else {
        Observation::NoFinding
    }
}

fn unused_sources(
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> impl Iterator<Item = &crate::insights::TokenBurnSourceEvidence> {
    source_evidence
        .and_then(|value| value.mcp_sources.as_ref())
        .into_iter()
        .flatten()
        .filter(|source| source.replicated_tokens > 0 && !source.invoked)
}

pub(super) fn finding_causes_with_source_evidence(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> Vec<FindingCause> {
    if !source_assessable(evidence, source_evidence) {
        return finding_causes(evidence);
    }
    let pricing_revision = source_evidence.and_then(|value| value.pricing_revision.clone());
    let mut causes = unused_sources(source_evidence)
        .map(|source| FindingCause::UnusedMcpServer {
            server: source.name.clone(),
            tokens: Some(source.replicated_tokens),
            cost_usd: source.replicated_cost_usd,
            pricing_revision: pricing_revision.clone(),
        })
        .collect::<Vec<_>>();
    causes.sort_by(|left, right| match (left, right) {
        (
            FindingCause::UnusedMcpServer { server: left, .. },
            FindingCause::UnusedMcpServer { server: right, .. },
        ) => left.cmp(right),
        _ => core::cmp::Ordering::Equal,
    });
    causes
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
        let mut evidence = with_server(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.mcp_servers.get_mut("server-a").unwrap().token_count = Some(123);
        assert_eq!(evaluate(&evidence), Observation::Finding);
        assert_eq!(
            finding_causes(&evidence),
            vec![FindingCause::UnusedMcpServer {
                server: "server-a".into(),
                tokens: None,
                cost_usd: None,
                pricing_revision: None,
            }]
        );
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

    fn unsupported_coverage_evidence() -> SessionEvidence {
        let mut evidence = with_server(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.mcp_coverage = EvidenceValue::Unsupported;
        evidence
    }

    #[test]
    fn source_attribution_produces_a_priced_cause_when_flat_coverage_is_unsupported() {
        let evidence = unsupported_coverage_evidence();
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.mcp_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "server-a".to_owned(),
            replicated_tokens: 300,
            invoked: false,
            replicated_cost_usd: Some(0.03),
        }]);
        source_evidence.pricing_revision = Some("pricing-generation-9".to_owned());

        assert!(source_assessable(&evidence, Some(&source_evidence)));
        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::Finding
        );
        assert_eq!(
            finding_causes_with_source_evidence(&evidence, Some(&source_evidence)),
            vec![FindingCause::UnusedMcpServer {
                server: "server-a".to_owned(),
                tokens: Some(300),
                cost_usd: Some(0.03),
                pricing_revision: Some("pricing-generation-9".to_owned()),
            }]
        );
    }

    #[test]
    fn source_attribution_is_not_assessable_when_flat_coverage_resolves() {
        // mcp_coverage is Complete here, so the flat path already applies
        // and source evidence must not override it.
        let evidence = with_server(false);
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.mcp_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "server-a".to_owned(),
            replicated_tokens: 300,
            invoked: false,
            replicated_cost_usd: Some(0.03),
        }]);

        assert!(!source_assessable(&evidence, Some(&source_evidence)));
        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            evaluate(&evidence)
        );
    }

    #[test]
    fn an_invoked_source_is_excluded_from_source_attribution() {
        let evidence = unsupported_coverage_evidence();
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.mcp_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "server-a".to_owned(),
            replicated_tokens: 300,
            invoked: true,
            replicated_cost_usd: Some(0.03),
        }]);

        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::NoFinding
        );
        assert!(finding_causes_with_source_evidence(&evidence, Some(&source_evidence)).is_empty());
    }

    #[test]
    fn a_zero_token_source_is_excluded_from_source_attribution() {
        let evidence = unsupported_coverage_evidence();
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.mcp_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "server-a".to_owned(),
            replicated_tokens: 0,
            invoked: false,
            replicated_cost_usd: None,
        }]);

        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::NoFinding
        );
        assert!(finding_causes_with_source_evidence(&evidence, Some(&source_evidence)).is_empty());
    }
}
