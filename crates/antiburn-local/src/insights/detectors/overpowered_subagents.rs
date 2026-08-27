//! Overpowered Subagents: premium main-loop models that spawn
//! subagents on the same premium tier.
//!
//! Claude delegated model evidence applies to the session. A sidechain root
//! does not identify its `Task` spawn, so the evidence does not invent an edge.
//!
//! Partial-evidence rules:
//! - Observed premium parent and delegated models permit a finding.
//! - Partial subagent evidence prevents clean when no finding exists.
//! - Missing parent or delegated model evidence reports the contract gap.

use crate::analysis::{EvidenceValue, SessionEvidence};
use crate::pricing::normalize_model_key;

use super::{Observation, ReportCatalogs, observed};

pub(crate) fn evaluate(evidence: &SessionEvidence, catalogs: &ReportCatalogs) -> Observation {
    let Some(subagents) = observed(&evidence.subagents) else {
        return Observation::NoFinding;
    };
    let has_subagent_activity = subagents.spawn_count > 0
        || subagents.delegated_turns > 0
        || !subagents.children.is_empty();
    if !has_subagent_activity {
        return Observation::NoFinding;
    }
    let group_is_partial = matches!(&evidence.subagents, EvidenceValue::Partial { .. });
    if subagents.delegated_models.is_empty() {
        return if group_is_partial {
            Observation::NoFinding
        } else {
            Observation::ContractIncomplete
        };
    }

    let has_premium_delegated_model = subagents
        .delegated_models
        .iter()
        .any(|model| is_premium(model, catalogs));
    if !has_premium_delegated_model {
        return Observation::NoFinding;
    }
    if subagents
        .children
        .iter()
        .filter_map(|child| child.parent_model.as_deref())
        .any(|model| is_premium(model, catalogs))
    {
        return Observation::Finding;
    }
    if subagents.children.is_empty()
        || subagents
            .children
            .iter()
            .any(|child| child.parent_model.is_none())
    {
        return if group_is_partial {
            Observation::NoFinding
        } else {
            Observation::ContractIncomplete
        };
    }
    Observation::NoFinding
}

fn is_premium(model: &str, catalogs: &ReportCatalogs) -> bool {
    catalogs.premium_models.contains(model)
        || catalogs.premium_models.contains(normalize_model_key(model))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{RelationConfidence, RelationProvenance, SubagentChild};

    fn evidence_with_models(
        parent_model: Option<&str>,
        delegated_models: &[&str],
    ) -> SessionEvidence {
        let mut evidence = claude_evidence("models");
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            unreachable!()
        };
        subagents.spawn_count = 1;
        subagents.delegated_turns = u64::try_from(delegated_models.len()).unwrap();
        subagents.delegated_models = delegated_models
            .iter()
            .map(|model| (*model).to_owned())
            .collect();
        subagents.children.push(SubagentChild {
            ordinal: 1,
            parent_model: parent_model.map(str::to_owned),
            child_model: EvidenceValue::Unsupported,
            confidence: RelationConfidence::Observed,
            provenance: RelationProvenance::TaskToolUse,
        });
        evidence
    }

    #[test]
    fn zero_spawns_report_no_finding() {
        assert_eq!(
            evaluate(&claude_evidence("no-spawns"), &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn observed_spawns_without_delegated_models_report_the_contract_gap() {
        let mut evidence = claude_evidence("spawns");
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            unreachable!()
        };
        subagents.spawn_count = 1;

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn delegated_turns_without_spawn_records_report_the_contract_gap() {
        let mut evidence = claude_evidence("delegated-only");
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            unreachable!()
        };
        subagents.delegated_turns = 1;
        subagents
            .delegated_models
            .insert("claude-opus-4-6".to_owned());

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn premium_parent_and_delegated_models_are_a_finding() {
        let evidence = evidence_with_models(Some("claude-opus-4-6"), &["claude-opus-4-7-20260115"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn lower_cost_delegated_models_report_no_finding() {
        let evidence = evidence_with_models(Some("claude-opus-4-6"), &["claude-sonnet-4-6"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn lower_cost_parent_models_report_no_finding() {
        let evidence = evidence_with_models(Some("claude-sonnet-4-6"), &["claude-opus-4-6"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_missing_parent_model_reports_the_contract_gap() {
        let evidence = evidence_with_models(None, &["claude-opus-4-6"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
        );
    }
}
