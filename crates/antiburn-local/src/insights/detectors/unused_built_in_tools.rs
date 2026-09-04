//! Unused Built-In Tools: native harness tool definitions that consume
//! context and are never used.
//!
//! The finding is an absence claim about invocation: a definition
//! occupies the session's context but the transcript never calls it.
//! Absence can only be read from a complete, named catalogue, so partial
//! or missing evidence never permits a finding.
//!
//! Partial-evidence rules:
//! - No partial evidence permits a finding. The rule needs complete
//!   `context_sources`, `tools`, and `eligibility` groups, and needs the
//!   nested `tool_definitions` map itself `Complete` (not `Partial`): a
//!   partial map may have missed the invoking record, and a
//!   never-invoked flag from it would be a false positive.
//! - `tool_definitions` reporting `Unsupported` inside an otherwise
//!   complete group is not an absence claim at all — the source
//!   supports the fact (`capabilities.tool_definitions`), but this
//!   session's harness version or model did not resolve against the
//!   built-in tool catalogue. That reports `SignalMissing`, not clean
//!   and not a finding.
//! - Partial evidence in any required group prevents clean.
//!
//! Exclusions: a situational tool (one that enters the request only
//! when used, such as `Skill` or `enter_plan_mode`) carries no idle
//! context cost and can never be an honest finding, so it is excluded
//! regardless of its `invoked` flag. A deferred definition never sent
//! its full token cost either, so it is excluded too. A zero-token
//! definition has nothing to reclaim.

use std::collections::BTreeMap;

use crate::analysis::tool_catalog::{comparable_tool_name, situational_tools};
use crate::analysis::{EvidenceValue, SessionEvidence, ToolDefinition};

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::CoverageReason;

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
}
