// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Sessions Over Depth: individual requests whose observed context
//! depth exceeds the report-time cap.
//!
//! Partial-evidence rules:
//! - Partial context evidence still permits a finding. An observed
//!   over-cap request proves presence.
//! - Partial context evidence prevents clean. A missed record may hide
//!   a deeper request, so absence cannot be concluded.

use crate::analysis::SessionEvidence;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    if let Some(context) = observed(&evidence.context)
        && context.max_request_context_tokens > catalogs.depth_cap_tokens
    {
        return Observation::Finding;
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{ContextEvidence, CoverageReason, EvidenceValue};

    fn with_depth(depth: u64, partial: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("depth");
        let context = ContextEvidence {
            max_request_context_tokens: depth,
            top_depth_examples: Vec::new(),
        };
        evidence.context = if partial {
            EvidenceValue::Partial {
                observed: context,
                reason: CoverageReason::MalformedRecord,
            }
        } else {
            EvidenceValue::Complete(context)
        };
        evidence
    }

    #[test]
    fn over_cap_depth_is_a_finding_even_from_partial_evidence() {
        let catalogs = ReportCatalogs::default();
        let depth = catalogs.depth_cap_tokens + 1;

        assert_eq!(
            evaluate(&with_depth(depth, false), &catalogs),
            Observation::Finding
        );
        assert_eq!(
            evaluate(&with_depth(depth, true), &catalogs),
            Observation::Finding
        );
    }

    #[test]
    fn depth_at_or_below_the_cap_is_no_finding() {
        let catalogs = ReportCatalogs::default();

        assert_eq!(
            evaluate(&with_depth(catalogs.depth_cap_tokens, false), &catalogs),
            Observation::NoFinding
        );
    }
}
