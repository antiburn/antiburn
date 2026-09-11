//! Finds premium parent and child models linked by a native task.
//! Aggregate models do not prove ancestry, including in older persisted evidence.

use crate::analysis::SessionEvidence;
use crate::pricing::canonical_model_key;
use crate::remediation::FindingCause;

use super::{Observation, ReportCatalogs, model_family, observed};

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
    let mut incomplete = subagents.children.is_empty();
    for child in &subagents.children {
        if child.observed_child_models.is_empty() {
            incomplete = true;
            continue;
        }
        let parent = child
            .parent_model
            .as_deref()
            .and_then(|model| premium_verdict(model, catalogs));
        for model in &child.observed_child_models {
            match (parent, premium_verdict(model, catalogs)) {
                (Some(true), Some(true)) => return Observation::Finding,
                (Some(_), Some(_)) => {}
                _ => incomplete = true,
            }
        }
    }
    if incomplete {
        if matches!(
            evidence.subagents,
            crate::analysis::EvidenceValue::Partial { .. }
        ) {
            Observation::NoFinding
        } else {
            Observation::ContractIncomplete
        }
    } else {
        Observation::NoFinding
    }
}

pub(super) fn finding_causes(
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Vec<FindingCause> {
    let Some(subagents) = observed(&evidence.subagents) else {
        return Vec::new();
    };
    let mut causes = Vec::new();
    for child in &subagents.children {
        let Some(parent_model) = child.parent_model.as_ref() else {
            continue;
        };
        if premium_verdict(parent_model, catalogs) != Some(true) {
            continue;
        }
        for worker_model in &child.observed_child_models {
            if premium_verdict(worker_model, catalogs) == Some(true) {
                causes.push(FindingCause::OverpoweredSubagents {
                    parent_model: parent_model.clone(),
                    worker_model: worker_model.clone(),
                    worker_ordinal: child.ordinal,
                    parent_call_id: child.parent_call_id.clone(),
                });
            }
        }
    }
    causes
}

/// One model's premium verdict under its family's reviewed policy.
/// `None` means the family's premium policy is not reviewed, so the
/// verdict is unknown — not "not premium".
fn premium_verdict(model: &str, catalogs: &ReportCatalogs) -> Option<bool> {
    let family = model_family(model);
    let policy = &catalogs.families.get(&family)?.premium;
    if !policy.reviewed {
        return None;
    }
    let canonical = canonical_model_key(model);
    Some(policy.is_premium(&canonical))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{
        EvidenceValue, ModelTokens, RelationConfidence, RelationProvenance, SubagentChild,
    };

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
            parent_call_id: Some("call-1".to_owned()),
            observed_child_models: subagents.delegated_models.clone(),
            child_model: EvidenceValue::Unsupported,
            confidence: RelationConfidence::Observed,
            provenance: RelationProvenance::TaskToolUse,
        });
        evidence
    }

    fn with_dominant_main_model(mut evidence: SessionEvidence, model: &str) -> SessionEvidence {
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            unreachable!()
        };
        models.dominant_main_model = Some(model.to_owned());
        models
            .by_model
            .insert(model.to_owned(), ModelTokens::default());
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
    fn premium_parent_and_child_models_are_a_finding_when_paired() {
        let evidence = evidence_with_models(Some("claude-opus-4-6"), &["claude-opus-4-7-20260115"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn a_dominant_main_model_does_not_replace_the_actual_parent() {
        let evidence = with_dominant_main_model(
            evidence_with_models(Some("claude-sonnet-4-6"), &["claude-opus-4-6"]),
            "claude-opus-4-6",
        );

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn legacy_aggregates_do_not_prove_ancestry() {
        let mut evidence = with_dominant_main_model(
            evidence_with_models(Some("claude-opus-4-6"), &["claude-opus-4-6"]),
            "claude-opus-4-6",
        );
        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            unreachable!()
        };
        subagents.children[0].observed_child_models.clear();
        subagents.children[0].parent_call_id = None;
        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
        );
    }

    #[test]
    fn a_lower_cost_dominant_model_does_not_hide_a_premium_pair() {
        let evidence = with_dominant_main_model(
            evidence_with_models(Some("claude-opus-4-6"), &["claude-opus-4-6"]),
            "claude-sonnet-4-6",
        );
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

    #[test]
    fn an_openai_premium_parent_and_child_are_a_finding() {
        let evidence = with_dominant_main_model(
            evidence_with_models(Some("gpt-5.6-sol"), &["gpt-5.6-sol"]),
            "gpt-5.6-sol",
        );

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn an_astra_parent_and_child_are_a_finding() {
        let evidence = with_dominant_main_model(
            evidence_with_models(Some("gpt-6-astra"), &["gpt-6-astra"]),
            "gpt-6-astra",
        );

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn an_openai_non_premium_luna_child_reports_no_finding() {
        let evidence = with_dominant_main_model(
            evidence_with_models(Some("gpt-5.6-sol"), &["gpt-5.6-luna"]),
            "gpt-5.6-sol",
        );

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::NoFinding
        );
    }

    #[test]
    fn a_gemini_pro_delegate_is_premium() {
        let evidence = evidence_with_models(Some("claude-opus-4-6"), &["gemini-3.1-pro"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn an_antigravity_prefixed_gemini_pro_delegate_is_premium() {
        let evidence = evidence_with_models(
            Some("claude-opus-4-6"),
            &["antigravity-gemini-3.8-pro-preview"],
        );

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn premium_verdict_flags_bare_gpt_5_6() {
        assert_eq!(
            premium_verdict("gpt-5.6", &ReportCatalogs::default()),
            Some(true)
        );
    }

    #[test]
    fn premium_verdict_flags_gpt_5_5_fast() {
        assert_eq!(
            premium_verdict("gpt-5.5-fast", &ReportCatalogs::default()),
            Some(true)
        );
    }

    #[test]
    fn premium_verdict_excepts_gpt_5_6_terra() {
        assert_eq!(
            premium_verdict("gpt-5.6-terra", &ReportCatalogs::default()),
            Some(false)
        );
    }

    #[test]
    fn premium_verdict_excepts_gpt_5_6_luna() {
        assert_eq!(
            premium_verdict("gpt-5.6-luna", &ReportCatalogs::default()),
            Some(false)
        );
    }

    #[test]
    fn premium_verdict_flags_claude_mythos_5() {
        assert_eq!(
            premium_verdict("claude-mythos-5", &ReportCatalogs::default()),
            Some(true)
        );
    }

    #[test]
    fn premium_verdict_does_not_flag_claude_sonnet_5() {
        assert_eq!(
            premium_verdict("claude-sonnet-5", &ReportCatalogs::default()),
            Some(false)
        );
    }

    #[test]
    fn premium_verdict_does_not_flag_a_date_suffixed_claude_haiku() {
        assert_eq!(
            premium_verdict("claude-haiku-4-5-20251001", &ReportCatalogs::default()),
            Some(false)
        );
    }
}
