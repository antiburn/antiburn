// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Old Model Usage: turns on a curated deprecated model after its
//! reviewed replacement became available.
//!
//! The replacement catalog is a report-time input keyed by normalized
//! model name. Evidence carries only model identity, quantities, and
//! timestamps (Locked Decision 2).
//!
//! Partial-evidence rules:
//! - Partial model evidence still permits a finding. Observed turns on
//!   a deprecated model prove presence.
//! - Partial model evidence prevents clean. A missed record may hide
//!   deprecated usage.

use crate::analysis::SessionEvidence;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    if let Some(models) = observed(&evidence.models) {
        for (model, tokens) in &models.by_model {
            if let Some(replacement) = catalogs.model_replacements.get(model)
                && tokens.turns > 0
                && tokens.last_ts_ms >= replacement.available_since_ts_ms
            {
                return Observation::Finding;
            }
        }
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::ModelReplacement;
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, ModelTokens};

    fn catalogs() -> ReportCatalogs {
        let mut catalogs = ReportCatalogs::default();
        catalogs.model_replacements.insert(
            "old-model-1".to_owned(),
            ModelReplacement {
                replacement: "new-model-2".to_owned(),
                available_since_ts_ms: 100,
            },
        );
        catalogs
    }

    fn with_model(model: &str, last_ts_ms: i64, partial: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("old-model");
        let EvidenceValue::Complete(mut models) = evidence.models else {
            unreachable!()
        };
        models.by_model.insert(
            model.to_owned(),
            ModelTokens {
                turns: 4,
                last_ts_ms,
                ..ModelTokens::default()
            },
        );
        evidence.models = if partial {
            EvidenceValue::Partial {
                observed: models,
                reason: CoverageReason::IncompleteTail,
            }
        } else {
            EvidenceValue::Complete(models)
        };
        evidence
    }

    #[test]
    fn deprecated_model_after_replacement_is_a_finding_even_from_partial_evidence() {
        assert_eq!(
            evaluate(&with_model("old-model-1", 200, false), &catalogs()),
            Observation::Finding
        );
        assert_eq!(
            evaluate(&with_model("old-model-1", 200, true), &catalogs()),
            Observation::Finding
        );
    }

    #[test]
    fn usage_before_the_replacement_shipped_is_no_finding() {
        assert_eq!(
            evaluate(&with_model("old-model-1", 50, false), &catalogs()),
            Observation::NoFinding
        );
    }

    #[test]
    fn uncatalogued_model_is_no_finding() {
        assert_eq!(
            evaluate(&with_model("new-model-2", 200, false), &catalogs()),
            Observation::NoFinding
        );
    }
}
