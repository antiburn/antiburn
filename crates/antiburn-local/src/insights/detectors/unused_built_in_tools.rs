//! Unused Built-In Tools: native harness tool definitions that consume
//! context and are never used.
//!
//! The finding is an absence claim about invocation: a definition
//! occupies the session's context but the transcript never calls it.
//! Findings require complete invocation coverage for the named definitions.
//! The catalogue does not prove the full historical inventory, so the report cannot claim clean.
//!
//! Partial-evidence rules:
//! - No partial evidence permits a finding. The rule needs complete
//!   `tools` and `eligibility` groups, and needs the
//!   nested `tool_definitions` map itself `Complete` (not `Partial`): a
//!   partial map may have missed the invoking record, and a
//!   never-invoked flag from it would be a false positive.
//! - `tool_definitions` reporting `Unsupported` inside an otherwise
//!   complete group is not an absence claim at all — the source
//!   supports the fact (`capabilities.tool_definitions`), but this
//!   session's harness version or model did not resolve against the
//!   built-in tool catalogue. That reports `SignalMissing`, not clean
//!   and not a finding.
//! - An unrelated partial resource group does not block a finding.
//!
//! Exclusions: a situational tool (one that enters the request only
//! when used, such as `Skill` or `enter_plan_mode`) carries no idle
//! context cost and can never be an honest finding, so it is excluded
//! regardless of its `invoked` flag. A deferred definition never sent
//! its full token cost either, so it is excluded too. A zero-token
//! definition has nothing to reclaim.

use std::collections::BTreeMap;

use crate::analysis::tool_catalog::{comparable_tool_name, situational_tools};
use crate::analysis::{EvidenceCoverage, EvidenceValue, SessionEvidence, ToolDefinition};
use crate::insights::SessionTokenBurnEvidence;
use crate::insights::report::{Fact, FactState};
use crate::remediation::{BuiltInToolTokens, FindingCause};

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
    match &sources.tool_definitions {
        EvidenceValue::Unsupported => Observation::SignalMissing,
        EvidenceValue::Partial { .. } => Observation::NoFinding,
        EvidenceValue::Complete(definitions) => {
            if has_unused_definition(&evidence.identity.agent, definitions) {
                Observation::Finding
            } else {
                Observation::NoFinding
            }
        }
    }
}

pub(super) fn source_assessable(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> bool {
    source_evidence
        .and_then(|value| value.built_in_tool_sources.as_ref())
        .is_some()
        && Fact::ToolDefinitions.state(evidence) == FactState::Unsupported
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
    if unused_sources(evidence, source_evidence).next().is_some() {
        Observation::Finding
    } else {
        Observation::NoFinding
    }
}

/// True when `definitions` names at least one built-in tool that costs
/// real context, was not deferred, was never invoked, and is not on
/// `agent`'s situational list.
fn has_unused_definition(agent: &str, definitions: &BTreeMap<String, ToolDefinition>) -> bool {
    let situational: Vec<String> = situational_tools(agent)
        .iter()
        .map(|name| comparable_tool_name(name))
        .collect();
    definitions.iter().any(|(name, definition)| {
        definition.tokens > 0
            && !definition.deferred
            && !definition.invoked
            && !situational.contains(&comparable_tool_name(name))
    })
}

pub(super) fn finding_causes(evidence: &SessionEvidence) -> Vec<FindingCause> {
    let Some(sources) = super::observed(&evidence.context_sources) else {
        return Vec::new();
    };
    let Some(definitions) = super::complete(&sources.tool_definitions) else {
        return Vec::new();
    };
    let situational: Vec<String> = situational_tools(&evidence.identity.agent)
        .iter()
        .map(|name| comparable_tool_name(name))
        .collect();
    definitions
        .iter()
        .filter(|(name, definition)| {
            definition.tokens > 0
                && !definition.deferred
                && !definition.invoked
                && !situational.contains(&comparable_tool_name(name))
        })
        .map(|(tool, definition)| FindingCause::UnusedBuiltInTool {
            tool: tool.clone(),
            tokens: BuiltInToolTokens::Definition(u64::from(definition.tokens)),
        })
        .collect()
}

pub(super) fn finding_causes_with_source_evidence(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> Vec<FindingCause> {
    if !source_assessable(evidence, source_evidence) {
        return finding_causes(evidence);
    }
    let mut causes = unused_sources(evidence, source_evidence)
        .map(|source| FindingCause::UnusedBuiltInTool {
            tool: source.name.clone(),
            tokens: BuiltInToolTokens::Replicated(source.replicated_tokens),
        })
        .collect::<Vec<_>>();
    causes.sort_by(|left, right| match (left, right) {
        (
            FindingCause::UnusedBuiltInTool { tool: left, .. },
            FindingCause::UnusedBuiltInTool { tool: right, .. },
        ) => left.cmp(right),
        _ => core::cmp::Ordering::Equal,
    });
    causes
}

fn unused_sources<'a>(
    evidence: &'a SessionEvidence,
    source_evidence: Option<&'a SessionTokenBurnEvidence>,
) -> impl Iterator<Item = &'a crate::insights::TokenBurnSourceEvidence> {
    let situational = situational_tools(&evidence.identity.agent);
    source_evidence
        .and_then(|value| value.built_in_tool_sources.as_ref())
        .into_iter()
        .flatten()
        .filter(move |source| {
            source.replicated_tokens > 0
                && !source.invoked
                && !situational
                    .iter()
                    .any(|name| comparable_tool_name(name) == comparable_tool_name(&source.name))
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::CoverageReason;
    use crate::insights::TokenBurnSourceEvidence;

    fn unused(tokens: u32) -> ToolDefinition {
        ToolDefinition {
            tokens,
            invoked: false,
            deferred: false,
        }
    }

    fn with_definition(name: &str, definition: ToolDefinition) -> SessionEvidence {
        let mut evidence = claude_evidence("built-in");
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 3;
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        let mut definitions = BTreeMap::new();
        definitions.insert(name.to_owned(), definition);
        sources.tool_definitions = EvidenceValue::Complete(definitions);
        evidence
    }

    #[test]
    fn an_unused_definition_is_a_finding() {
        assert_eq!(
            evaluate(&with_definition("bash", unused(100))),
            Observation::Finding
        );
    }

    #[test]
    fn incomplete_resource_inventory_does_not_block_built_in_tools() {
        let mut evidence = with_definition("bash", unused(100));
        let EvidenceValue::Complete(sources) = evidence.context_sources else {
            unreachable!()
        };
        evidence.context_sources = EvidenceValue::Partial {
            observed: sources,
            reason: CoverageReason::AttributionIncomplete,
        };
        assert_eq!(evaluate(&evidence), Observation::Finding);
    }

    #[test]
    fn an_invoked_definition_is_no_finding() {
        let definition = ToolDefinition {
            invoked: true,
            ..unused(100)
        };
        assert_eq!(
            evaluate(&with_definition("bash", definition)),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_deferred_definition_is_no_finding() {
        let definition = ToolDefinition {
            deferred: true,
            ..unused(100)
        };
        assert_eq!(
            evaluate(&with_definition("bash", definition)),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_situational_definition_is_no_finding() {
        assert_eq!(
            evaluate(&with_definition("skill", unused(50))),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_zero_token_definition_is_no_finding() {
        assert_eq!(
            evaluate(&with_definition("bash", unused(0))),
            Observation::NoFinding
        );
    }

    #[test]
    fn partial_tools_coverage_never_claims_the_absence_finding() {
        let mut evidence = with_definition("bash", unused(100));
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
    fn unsupported_definitions_report_the_signal_gap() {
        let mut evidence = with_definition("bash", unused(100));
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.tool_definitions = EvidenceValue::Unsupported;
        assert_eq!(evaluate(&evidence), Observation::SignalMissing);
    }

    #[test]
    fn session_without_assistant_work_is_no_finding() {
        let mut evidence = with_definition("bash", unused(100));
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 0;
        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }

    #[test]
    fn source_attribution_produces_a_replicated_token_cause() {
        let mut evidence = with_definition("bash", unused(100));
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.tool_definitions = EvidenceValue::Unsupported;
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.built_in_tool_sources = Some(vec![TokenBurnSourceEvidence {
            scope: "agent:bundled".to_owned(),
            name: "Write".to_owned(),
            replicated_tokens: u128::from(u64::MAX) + 9,
            invoked: false,
        }]);

        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::Finding
        );
        assert_eq!(
            finding_causes_with_source_evidence(&evidence, Some(&source_evidence)),
            vec![FindingCause::UnusedBuiltInTool {
                tool: "Write".to_owned(),
                tokens: BuiltInToolTokens::Replicated(u128::from(u64::MAX) + 9),
            }]
        );
    }
}
