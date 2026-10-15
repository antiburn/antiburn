use super::models::*;
use antiburn_local::remediation::FindingCause;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn bounded_evidence_excerpt(text: &str) -> String {
    let end = text
        .char_indices()
        .take_while(|(index, character)| *index + character.len_utf8() <= 4096)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    text[..end].to_owned()
}

pub(super) fn available_evidence_references(
    action_id: &str,
    context_ids: &BTreeSet<&str>,
    available_ids: &BTreeSet<String>,
) -> Option<(Vec<String>, bool)> {
    if !available_ids.contains(action_id) {
        return None;
    }
    let missing_context = context_ids
        .iter()
        .any(|reference| !available_ids.contains(*reference));
    let mut references = vec![action_id.to_owned()];
    references.extend(
        context_ids
            .iter()
            .filter(|reference| available_ids.contains(**reference))
            .map(|reference| (*reference).to_owned()),
    );
    Some((references, missing_context))
}

pub(super) fn explanation_quote(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = text.chars();
    let selected = characters.by_ref().take(180).collect::<String>();
    format!(
        "“{selected}{}”",
        if characters.next().is_some() {
            "…"
        } else {
            ""
        }
    )
}

pub(super) fn instruction_comparison(
    cause: &FindingCause,
    evidence: &BurnCheckTargetEvidence,
) -> Option<BurnCheckEvidenceComparison> {
    let FindingCause::IgnoredInstructionConflict(finding) = cause else {
        return None;
    };
    let decision = finding.decision_record()?;
    let basis = decision.explanation_basis.as_ref()?;
    if evidence.status != BurnCheckEvidenceStatus::Available
        || evidence.decision_proof.is_none()
        || basis.schema_revision != 1
    {
        return None;
    }
    let instruction = evidence.items.iter().find(|item| {
        item.label == BurnCheckEvidenceLabel::Instruction && item.excerpt == basis.instruction
    })?;
    let action = evidence.items.iter().find(|item| {
        item.label == BurnCheckEvidenceLabel::ObservedAction && item.excerpt == basis.action
    })?;
    let verb = if decision.action_is_request {
        "requested"
    } else {
        "recorded"
    };
    let instruction_label = if finding.provenance == antiburn_local::analysis::ignored_instructions::InstructionProvenance::CurrentFileComparison { "The current instruction" } else { "The instruction" };
    use antiburn_local::analysis::ignored_instructions::PrerequisiteOutcome;
    let mut references = vec![instruction.reference.clone(), action.reference.clone()];
    let action_clause = match decision.prerequisite {
        PrerequisiteOutcome::NotRequired => format!(
            "The agent {verb} {}, which conflicts with that requirement.",
            explanation_quote(&basis.action)
        ),
        PrerequisiteOutcome::EarlierRequestAbsent => format!(
            "The agent {verb} {} without a preceding required read request in the recorded request history.",
            explanation_quote(&basis.action)
        ),
        PrerequisiteOutcome::SelectedHistoryConflict => {
            let earlier = evidence.items.iter().find(|item| {
                item.label == BurnCheckEvidenceLabel::Context
                    && basis.earlier_events.iter().any(|event| {
                        event.action_id == item.reference && event.text == item.excerpt
                    })
            })?;
            references.push(earlier.reference.clone());
            format!(
                "Selected earlier events include {}; the agent then {verb} {}, which conflicts with the required sequence.",
                explanation_quote(&earlier.excerpt),
                explanation_quote(&basis.action)
            )
        }
    };
    Some(BurnCheckEvidenceComparison {
        source_ranges: vec![
            BurnCheckSourceRange {
                reference: instruction.reference.clone(),
                start_byte: decision.rule_start_byte,
                end_byte: decision.rule_end_byte,
                range_source: "instruction".into(),
            },
            BurnCheckSourceRange {
                reference: action.reference.clone(),
                start_byte: decision.action_anchor.start_byte,
                end_byte: decision.action_anchor.end_byte,
                range_source: "selected_action_text".into(),
            },
        ],
        explanation: Some(BurnCheckExplanation {
            version: 1,
            relationship: "requirementConflict".into(),
            text: format!(
                "{instruction_label} requires {}. {action_clause}",
                explanation_quote(&basis.instruction),
            ),
            references,
        }),
        ..Default::default()
    })
}

pub(super) fn validated_read_task_items(
    decision: &antiburn_local::checks::over_exploring::Decision,
    scope: &antiburn_local::analysis::session_scope::SessionScopeSnapshot,
    actions: &[antiburn_local::analysis::jev_evidence::ContentAction],
) -> Option<Vec<BurnCheckEvidenceItem>> {
    use antiburn_local::analysis::session_scope::ScopeAuthority;
    let context = scope.user_context();
    if !decision.task_evidence_is_bound_to(&context.evidence) {
        return None;
    }
    let mut items = BTreeMap::new();
    for binding in &decision.task_evidence {
        let occurrence = scope
            .occurrences()
            .iter()
            .find(|occurrence| occurrence.reference.id == binding.source_id)?;
        let action = actions
            .iter()
            .find(|action| action.reference == occurrence.reference)?;
        if !action.reference.stable || occurrence.reference.id != binding.source_id {
            return None;
        }
        let value = scope.values().get(occurrence.value_index)?;
        let text = value.as_str()?;
        if text != action.text {
            return None;
        }
        let excerpt = bounded_evidence_excerpt(text);
        let requested = occurrence.authority == ScopeAuthority::User
            && action.authority == "user"
            && action.turn_role == "user";
        items.insert(
            binding.source_id.clone(),
            BurnCheckEvidenceItem {
                label: BurnCheckEvidenceLabel::Context,
                source_label: if requested {
                    "Requested task"
                } else {
                    "Recorded task context"
                }
                .into(),
                reference: occurrence.reference.id.clone(),
                observed_at_ms: action.timestamp_ms,
                start_line: None,
                end_line: None,
                excerpt,
                explanation: String::new(),
                limitation: None,
            },
        );
    }
    Some(items.into_values().collect())
}

pub(super) fn over_exploring_explanation(
    decision: &antiburn_local::checks::over_exploring::Decision,
    actions: &[antiburn_local::analysis::jev_evidence::ContentAction],
    items: &mut Vec<BurnCheckEvidenceItem>,
    reads: &[BurnCheckReadEvidence],
) -> Option<BurnCheckExplanation> {
    let basis = decision.explanation.as_ref()?;
    if !decision.has_valid_explanation() {
        return None;
    }
    if basis
        .snippets
        .iter()
        .any(|snippet| !actions.iter().any(|action| snippet.matches_action(action)))
        || basis
            .purpose
            .as_ref()
            .is_some_and(|purpose| !actions.iter().any(|action| purpose.matches_action(action)))
    {
        return None;
    }
    let mut selected = BTreeMap::<String, Vec<String>>::new();
    for snippet in &basis.snippets {
        selected
            .entry(snippet.reference.id.clone())
            .or_default()
            .push(snippet.text.clone());
    }
    if let Some(purpose) = &basis.purpose {
        selected
            .entry(purpose.reference.id.clone())
            .or_default()
            .push(purpose.text.clone());
    }
    for reference in &decision.task_evidence {
        let action = actions.iter().find(|action| {
            action.reference.id == reference.source_id
                && action.kind == "user"
                && action.authority == "user"
                && !action.truncated
        })?;
        selected
            .entry(reference.source_id.clone())
            .or_insert_with(|| vec![action.text.clone()]);
    }
    let task_reference = decision
        .task_evidence
        .iter()
        .find(|reference| selected.contains_key(&reference.source_id))?
        .source_id
        .clone();
    let mut selected_items = items.clone();
    for (reference, excerpts) in selected {
        let text = excerpts.join("\n…\n");
        if let Some(item) = selected_items
            .iter_mut()
            .find(|item| item.reference == reference)
        {
            item.excerpt = text;
        } else {
            let action = actions
                .iter()
                .find(|action| action.reference.id == reference)?;
            selected_items.push(BurnCheckEvidenceItem {
                label: BurnCheckEvidenceLabel::Context,
                source_label: "Earlier recorded event".into(),
                reference,
                observed_at_ms: action.timestamp_ms,
                start_line: None,
                end_line: None,
                excerpt: text,
                explanation: String::new(),
                limitation: None,
            });
        }
    }
    let task = selected_items
        .iter()
        .find(|item| item.reference == task_reference && !item.excerpt.is_empty())?;
    let paths = reads
        .iter()
        .flat_map(|read| &read.paths)
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    if paths.is_empty() {
        return None;
    }
    use antiburn_local::checks::over_exploring::ReadRelationship;
    let text = match basis.relationship {
        ReadRelationship::SelectedReadUnrelated => {
            let sample = basis.snippets.first()?;
            if let Some(purpose) = &basis.purpose {
                format!(
                    "The recorded purpose {} and sampled content {} from {paths} show a separate activity from the requested task {}. The sample does not establish the purpose of the whole file.",
                    explanation_quote(&purpose.text),
                    explanation_quote(&sample.text),
                    explanation_quote(&task.excerpt),
                )
            } else {
                format!(
                    "The sampled content {} from {paths} suggests a separate activity from the requested task {}. The sample does not establish the purpose of the whole file.",
                    explanation_quote(&sample.text),
                    explanation_quote(&task.excerpt),
                )
            }
        }
        ReadRelationship::LaterReadRepeatsEarlier => {
            let equality = if basis.whole_output_equal == Some(true) {
                " The recorded outputs are identical."
            } else {
                ""
            };
            format!(
                "The later reads of {paths} returned identical recorded output with no observed intervening work for the requested task {}.{equality} The earlier result may be reusable; this does not prove unchanged filesystem contents or exact savings.",
                explanation_quote(&task.excerpt)
            )
        }
    };
    let references = selected_items
        .iter()
        .map(|item| item.reference.clone())
        .collect();
    *items = selected_items;
    Some(BurnCheckExplanation {
        version: 1,
        relationship: match basis.relationship {
            ReadRelationship::SelectedReadUnrelated => "selectedReadUnrelated",
            ReadRelationship::LaterReadRepeatsEarlier => "laterReadRepeatsEarlier",
        }
        .into(),
        text,
        references,
    })
}
