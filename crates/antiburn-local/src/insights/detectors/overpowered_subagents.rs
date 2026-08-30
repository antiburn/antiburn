//! Overpowered Subagents: premium main-loop models that spawn
//! subagents on the same premium tier.
//!
//! Claude delegated model evidence applies to the session. A sidechain root
//! does not identify its `Task` spawn, so the evidence does not invent an edge.
//!
//! The "main-loop model" is the dominant `scope='main'` model
//! (`ModelEvidence::dominant_main_model`, the most-active model by turn
//! count). Older evidence and harnesses that have not filled the field
//! yet fall back to `children[].parent_model`, folded the same
//! dominant way: any premium value wins.
//!
//! Premium status is judged under each model's own family's reviewed
//! premium policy (`ReportCatalogs::families`). An unreviewed family
//! (including `ModelFamily::Unknown`) can prove neither premium nor
//! non-premium for its models.
//!
//! Partial-evidence rules:
//! - Observed premium parent and delegated models permit a finding.
//! - Partial subagent evidence prevents clean when no finding exists.
//! - Missing parent or delegated model evidence reports the contract gap.
//! - A delegated or parent model whose family policy is not reviewed
//!   reports the contract gap instead of assuming non-premium, unless a
//!   finding is already proven from another observed model.

use crate::analysis::{EvidenceValue, SessionEvidence, SubagentEvidence};
use crate::pricing::canonical_model_key;

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
    let group_is_partial = matches!(&evidence.subagents, EvidenceValue::Partial { .. });
    if subagents.delegated_models.is_empty() {
        return if group_is_partial {
            Observation::NoFinding
        } else {
            Observation::ContractIncomplete
        };
    }

    let mut any_premium_delegate = false;
    let mut any_unreviewed_delegate = false;
    for model in &subagents.delegated_models {
        match premium_verdict(model, catalogs) {
            Some(true) => any_premium_delegate = true,
            Some(false) => {}
            None => any_unreviewed_delegate = true,
        }
    }
    if !any_premium_delegate {
        return if any_unreviewed_delegate {
            // A delegated model's family is not reviewed for premium
            // tier. It might be premium; the session cannot prove it
            // is not.
            Observation::ContractIncomplete
        } else {
            Observation::NoFinding
        };
    }

    let dominant_main_model =
        observed(&evidence.models).and_then(|models| models.dominant_main_model.as_deref());
    let parent_verdict = match dominant_main_model {
        Some(model) => premium_verdict(model, catalogs),
        None => fallback_parent_verdict(subagents, catalogs),
    };
    match parent_verdict {
        Some(true) => Observation::Finding,
        Some(false) => Observation::NoFinding,
        None => {
            if group_is_partial {
                Observation::NoFinding
            } else {
                Observation::ContractIncomplete
            }
        }
    }
}

/// Folds every child's `parent_model` into one dominant-style verdict
/// for when no `dominant_main_model` was computed. `Some(true)` (a
/// proven premium parent) wins; failing that, an unreviewed family
/// wins over a proven `Some(false)`; no parent model observed at all
/// reports `None`, the same as an unreviewed family.
fn fallback_parent_verdict(
    subagents: &SubagentEvidence,
    catalogs: &ReportCatalogs,
) -> Option<bool> {
    let mut any_true = false;
    let mut any_unreviewed = false;
    let mut any_observed = false;
    for model in subagents
        .children
        .iter()
        .filter_map(|child| child.parent_model.as_deref())
    {
        any_observed = true;
        match premium_verdict(model, catalogs) {
            Some(true) => any_true = true,
            Some(false) => {}
            None => any_unreviewed = true,
        }
    }
    if any_true {
        Some(true)
    } else if any_unreviewed || !any_observed {
        None
    } else {
        Some(false)
    }
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
    use crate::analysis::{ModelTokens, RelationConfidence, RelationProvenance, SubagentChild};

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
    fn premium_parent_and_delegated_models_are_a_finding_via_the_children_fallback() {
        let evidence = evidence_with_models(Some("claude-opus-4-6"), &["claude-opus-4-7-20260115"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::Finding
        );
    }

    #[test]
    fn a_dominant_main_model_wins_over_a_lower_cost_parent_child() {
        // The child's `parent_model` claims a non-premium parent, but
        // `dominant_main_model` (the real signal now) is premium.
        let evidence = with_dominant_main_model(
            evidence_with_models(Some("claude-sonnet-4-6"), &["claude-opus-4-6"]),
            "claude-opus-4-6",
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
    fn an_unreviewed_family_delegate_reports_the_contract_gap() {
        let evidence = evidence_with_models(Some("claude-opus-4-6"), &["gemini-3.1-pro"]);

        assert_eq!(
            evaluate(&evidence, &ReportCatalogs::default()),
            Observation::ContractIncomplete
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
