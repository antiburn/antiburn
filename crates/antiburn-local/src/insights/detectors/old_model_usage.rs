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
//! - Catalogued turns without an observed timestamp report the
//!   contract gap: the rule cannot place the usage relative to the
//!   replacement's availability, so the session never supports clean.
//!   An observed finding elsewhere in the session still wins.
//! - An empty replacement catalog reports the contract gap. No entry
//!   in the catalog can prove that no deprecated model ran.

use crate::analysis::SessionEvidence;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    if catalogs.model_replacements.is_empty() {
        return Observation::ContractIncomplete;
    }
    let mut missing_timestamp = false;
    if let Some(models) = observed(&evidence.models) {
        for (model, tokens) in &models.by_model {
            // The registry keys every source ID and alias by
            // `canonical_model_key`; match observed model strings the
            // same way so a provider prefix, a date suffix, or mixed
            // case does not miss the rule.
            let Some(replacement) = catalogs.model_replacements.lookup(model) else {
                continue;
            };
            if tokens.turns == 0 {
                continue;
            }
            if tokens.last_ts_ms == 0 {
                missing_timestamp = true;
                continue;
            }
            if tokens.last_ts_ms >= replacement.available_since_ts_ms {
                return Observation::Finding;
            }
        }
    }
    if missing_timestamp {
        return Observation::ContractIncomplete;
    }
    Observation::NoFinding
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::super::{ModelRegistry, ModelReplacementEntry};
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, ModelTokens};

    fn catalogs() -> ReportCatalogs {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            "old-model-1".to_owned(),
            ModelReplacementEntry {
                replacement: "new-model-2".to_owned(),
                available_since_ts_ms: 100,
                rationale: "test rule".to_owned(),
                source_url: "https://example.invalid/old-model-1".to_owned(),
            },
        );
        entries.insert(
            "old-model-3".to_owned(),
            ModelReplacementEntry {
                replacement: "new-model-4".to_owned(),
                available_since_ts_ms: 100,
                rationale: "test rule".to_owned(),
                source_url: "https://example.invalid/old-model-3".to_owned(),
            },
        );
        ReportCatalogs {
            model_replacements: ModelRegistry {
                revision: 1,
                entries,
            },
            ..ReportCatalogs::default()
        }
    }

    fn insert_model(evidence: &mut SessionEvidence, model: &str, last_ts_ms: i64) {
        let EvidenceValue::Complete(models) = &mut evidence.models else {
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

    #[test]
    fn timestampless_catalogued_turns_report_the_contract_gap() {
        assert_eq!(
            evaluate(&with_model("old-model-1", 0, false), &catalogs()),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn timestampless_uncatalogued_turns_are_no_finding() {
        assert_eq!(
            evaluate(&with_model("new-model-2", 0, false), &catalogs()),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_finding_wins_over_a_timestampless_catalogued_model() {
        let mut evidence = with_model("old-model-1", 0, false);
        insert_model(&mut evidence, "old-model-3", 200);

        assert_eq!(evaluate(&evidence, &catalogs()), Observation::Finding);
    }

    #[test]
    fn an_empty_replacement_registry_reports_the_contract_gap() {
        // An empty catalog can never prove absence of deprecated-model
        // usage, so the rule must not read as no-finding.
        let evidence = with_model("any-model", 200, false);
        let catalogs = ReportCatalogs {
            model_replacements: ModelRegistry::empty(),
            ..ReportCatalogs::default()
        };

        assert_eq!(
            evaluate(&evidence, &catalogs),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn the_default_registry_finds_claude_opus_4_8_after_opus_5_shipped() {
        let evidence = with_model("claude-opus-4-8-20260801", 1_784_851_200_001, false);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn the_default_registry_finds_gpt_5_5_after_gpt_5_6_shipped() {
        let evidence = with_model("GPT-5.5", 1_783_555_200_001, false);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn the_default_registry_reads_no_finding_before_the_effective_date() {
        let evidence = with_model("claude-opus-4-8", 1_784_851_199_999, false);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_current_model_is_no_finding_under_the_default_registry() {
        let evidence = with_model("claude-sonnet-5", 2_000_000_000_000, false);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }
}
