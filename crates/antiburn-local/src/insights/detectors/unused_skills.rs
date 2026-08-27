// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Unused Skills: skills loaded into eligible sessions and never
//! invoked.
//!
//! Findings are not grouped by installed/project/plugin/bundled origin
//! yet: `LoadedSource::origin` carries no classification.
//!
//! The finding is an absence claim about invocation, so partial
//! evidence never permits it: a partial tools or context-sources group
//! may have missed the invoking record.
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
    if sources.skills.values().any(|skill| !skill.invoked) {
        return Observation::Finding;
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, LoadedSource};

    fn with_skill(invoked: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("skills");
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 2;
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.skills.insert(
            "skill-a".to_owned(),
            LoadedSource {
                description: None,
                invoked,
                origin: EvidenceValue::Unsupported,
            },
        );
        evidence
    }

    #[test]
    fn loaded_and_never_invoked_skill_is_a_finding() {
        assert_eq!(evaluate(&with_skill(false)), Observation::Finding);
    }

    #[test]
    fn invoked_skill_is_no_finding() {
        assert_eq!(evaluate(&with_skill(true)), Observation::NoFinding);
    }

    #[test]
    fn partial_context_sources_never_claim_the_absence_finding() {
        let mut evidence = with_skill(false);
        evidence.context_sources = match evidence.context_sources {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::CapExceeded,
            },
            _ => unreachable!(),
        };

        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }
}
