use std::collections::BTreeSet;

use antiburn_local::analysis::JevCheck;
use antiburn_local::checks::ignored_instructions::AssessmentInput;

use super::super::{IgnoredInstructionsCheck, build_jev_context};

#[derive(Clone, Debug)]
pub(crate) struct SemanticReference {
    pub(crate) source: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) action_reference_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BindingIdentity {
    pub(crate) source: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) action_reference_id: String,
    pub(crate) rule_id: String,
    pub(crate) action_id: String,
}

pub(crate) fn export_bindings(
    input: &AssessmentInput,
    selected: &[SemanticReference],
) -> Result<(String, Vec<BindingIdentity>), String> {
    let context = build_jev_context(input).map_err(|error| format!("build context: {error:?}"))?;
    let plan = IgnoredInstructionsCheck
        .prepare(&context)
        .map_err(|error| format!("prepare check: {error:?}"))?;
    let mut output = Vec::with_capacity(selected.len());
    for semantic in selected {
        let action = input
            .content
            .actions
            .iter()
            .find(|action| action.reference.id == semantic.action_reference_id)
            .ok_or_else(|| format!("source action not found: {}", semantic.action_reference_id))?;
        let matching = plan
            .prepared
            .comparisons
            .iter()
            .filter(|comparison| {
                comparison.reference.source == semantic.source
                    && comparison.reference.start_line == semantic.start_line
                    && comparison.reference.end_line == semantic.end_line
                    && comparison.reference.action_id == action.reference.id
            })
            .collect::<Vec<_>>();
        if matching.is_empty() {
            return Err(
                "selected source reference resolved to 0 production comparisons".to_owned(),
            );
        }
        let identities = matching
            .iter()
            .map(|comparison| {
                (
                    comparison.reference.rule_id.clone(),
                    comparison.reference.action_id.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        if identities.len() != 1 {
            return Err(format!(
                "selected source reference resolved to {} distinct production binding pairs",
                identities.len()
            ));
        }
        let (rule_id, action_id) = identities
            .into_iter()
            .next()
            .expect("one distinct production binding pair was checked above");
        output.push(BindingIdentity {
            source: semantic.source.clone(),
            start_line: semantic.start_line,
            end_line: semantic.end_line,
            action_reference_id: semantic.action_reference_id.clone(),
            rule_id,
            action_id,
        });
    }
    let revisions = IgnoredInstructionsCheck.revisions();
    Ok((
        format!(
            "parser={};projection={};chunking={};questions={};reducer={}",
            antiburn_local::analysis::PARSER_REVISION,
            revisions.projection,
            revisions.chunking,
            revisions.questions,
            revisions.reducer
        ),
        output,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::super::{
        AssessmentInput, ContentAction, ContentEventReference, InstructionProvenance,
        InstructionScope, SessionContentEvidence, SourceFormat, snapshot_from_text,
    };
    use super::*;

    fn synthetic_input() -> AssessmentInput {
        let instruction = snapshot_from_text(
            "AGENTS.md",
            "# Rules\n- Keep the source offline.".to_owned(),
            InstructionProvenance::RecordedInjection,
            InstructionScope::Project,
        )
        .unwrap();
        let action = ContentAction {
            reference: ContentEventReference {
                id: "synthetic-action".to_owned(),
                source_key_digest: "source".to_owned(),
                thread_digest: "thread".to_owned(),
                turn_index: 1,
                native_record_id: Some("synthetic-record".to_owned()),
                part_index: 0,
                stable: true,
            },
            timestamp_ms: Some(1),
            turn_role: "assistant".to_owned(),
            turn_scope: "main".to_owned(),
            authority: "assistant".to_owned(),
            kind: "assistant_text".to_owned(),
            text: "I checked the source offline.".to_owned(),
            tool_name: None,
            tool_call_id: None,
            normalized_fields: None,
            metadata: Default::default(),
            truncated: false,
            context_only: false,
        };
        AssessmentInput {
            content: SessionContentEvidence {
                session_identity_digest: "synthetic-session".to_owned(),
                source_format: SourceFormat::ClaudeJsonl,
                publication_fence: 1,
                selected_input_digest: "synthetic-input".to_owned(),
                actions: vec![action],
                instructions: vec![instruction],
                complete: true,
                limitations: vec![],
                excluded_thinking_parts: 0,
                field_availability: vec![],
            },
            prior_history_complete: true,
            activity_after_ms: None,
            boundary_positions: Default::default(),
            source_generation: 1,
            source_fingerprint: None,
            incarnation: 1,
            comparison_after: None,
        }
    }

    #[test]
    fn exports_production_identity_for_independently_selected_synthetic_reference() {
        let input = synthetic_input();
        let selected = SemanticReference {
            source: "AGENTS.md".to_owned(),
            start_line: 2,
            end_line: 2,
            action_reference_id: "synthetic-action".to_owned(),
        };
        let (revision, first) = export_bindings(&input, std::slice::from_ref(&selected)).unwrap();
        let (same_revision, second) = export_bindings(&input, &[selected]).unwrap();
        assert_eq!(revision, same_revision);
        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert!(!first[0].rule_id.is_empty());
        assert_eq!(first[0].action_id, "synthetic-action");
    }

    #[test]
    fn repeated_action_ranges_resolve_to_one_exact_production_binding_pair() {
        let mut input = synthetic_input();
        input.content.actions[0].text = format!(
            "{}The required action remains in this source range.",
            "Unrelated source text. ".repeat(400)
        );
        let selected = SemanticReference {
            source: "AGENTS.md".to_owned(),
            start_line: 2,
            end_line: 2,
            action_reference_id: "synthetic-action".to_owned(),
        };
        let (_, identities) = export_bindings(&input, &[selected]).unwrap();
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].action_id, "synthetic-action");
    }
}
