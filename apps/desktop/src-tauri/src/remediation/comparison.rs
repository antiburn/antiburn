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

pub(super) fn selected_read_excerpt<'a>(
    fields: &'a serde_json::Value,
    part_id: &str,
) -> Option<(usize, usize, &'a str)> {
    let pointer = format!(
        "/{}",
        part_id.replace('[', "/").replace(']', "").replace('.', "/")
    );
    let selected = fields.pointer(&pointer)?;
    let record = if selected.is_string() {
        fields.pointer(pointer.strip_suffix("/text")?)?
    } else {
        selected
    };
    let start = record
        .get("range")
        .and_then(|range| range.get(0))
        .or_else(|| record.get("start"))?
        .as_u64()?;
    let end = record
        .get("range")
        .and_then(|range| range.get(1))
        .or_else(|| record.get("end"))?
        .as_u64()?;
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    let text = record.get("text")?.as_str()?;
    (start < end && text.len() == end - start).then_some((start, end, text))
}

pub(super) fn validated_read_task_items(
    decision: &antiburn_local::checks::over_exploring::Decision,
    scope: &antiburn_local::analysis::session_scope::SessionScopeSnapshot,
    actions: &[antiburn_local::analysis::jev_evidence::ContentAction],
) -> Option<Vec<BurnCheckEvidenceItem>> {
    use antiburn_local::analysis::jev::JevEvidenceRole;
    use antiburn_local::analysis::session_scope::ScopeAuthority;
    let context = scope.user_context();
    let mut items = BTreeMap::new();
    for binding in &decision.task_evidence {
        let compact_index = binding
            .part_id
            .strip_prefix("task[")
            .and_then(|value| value.strip_suffix(']'))
            .and_then(|value| value.parse::<usize>().ok());
        let compact_selected = compact_index.and_then(|index| {
            let basis = decision.explanation.as_ref()?;
            let selected = basis.compared.fields.get("task")?.get(index)?;
            let source_index = usize::try_from(selected.get("at")?.as_u64()?).ok()?;
            let action = actions.get(source_index)?;
            (action.reference.id == binding.source_id).then_some((selected, action))
        });
        let (occurrence, action) = if let Some((_, action)) = compact_selected {
            let occurrence = scope
                .occurrences()
                .iter()
                .find(|occurrence| occurrence.reference == action.reference)?;
            (occurrence, action)
        } else {
            let occurrence = scope
                .occurrences()
                .iter()
                .find(|occurrence| occurrence.reference.id == binding.source_id)?;
            let action = actions
                .iter()
                .find(|action| action.reference == occurrence.reference)?;
            (occurrence, action)
        };
        if !action.reference.stable || occurrence.reference.id != binding.source_id {
            return None;
        }
        let value = scope.values().get(occurrence.value_index)?;
        let text = value.as_str()?;
        if text != action.text {
            return None;
        }
        let excerpt = if context
            .evidence
            .iter()
            .any(|reference| reference == binding)
        {
            bounded_evidence_excerpt(text)
        } else {
            let basis = decision.explanation.as_ref()?;
            if basis.version != 1
                || !basis.task.evidence.contains(binding)
                || !basis.compared.evidence.contains(binding)
                || binding.role != JevEvidenceRole::Instruction
                || binding.content_kind != action.kind
                || occurrence.authority != ScopeAuthority::User
                || action.authority != "user"
                || action.turn_role != "user"
            {
                return None;
            }
            let index = compact_index?;
            if binding.part_id != format!("task[{index}]") {
                return None;
            }
            let selected = basis.compared.fields.get("task")?.get(index)?;
            let source_index = usize::try_from(selected.get("at")?.as_u64()?).ok()?;
            if actions.get(source_index)?.reference != occurrence.reference {
                return None;
            }
            let (start, end, passage) =
                selected_read_excerpt(&basis.compared.fields, &binding.part_id)?;
            if !basis.snippets.iter().any(|snippet| {
                snippet.reference == occurrence.reference
                    && snippet.range == (start, end)
                    && snippet.text == passage
                    && snippet.matches_action(action)
            }) {
                return None;
            }
            passage.to_owned()
        };
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
    if basis.version != 1 {
        return None;
    }
    if basis
        .snippets
        .iter()
        .any(|snippet| !actions.iter().any(|action| snippet.matches_action(action)))
    {
        return None;
    }
    let mut selected = BTreeMap::<String, Vec<String>>::new();
    for reference in &basis.compared.evidence {
        let (start, end, text) = selected_read_excerpt(&basis.compared.fields, &reference.part_id)?;
        let action = actions
            .iter()
            .find(|action| action.reference.id == reference.source_id)?;
        if !basis.snippets.iter().any(|snippet| {
            snippet.reference == action.reference
                && snippet.range == (start, end)
                && snippet.text == text
                && snippet.matches_action(action)
        }) {
            return None;
        }
        selected
            .entry(reference.source_id.clone())
            .or_default()
            .push(text.into());
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
        ReadRelationship::SelectedReadUnrelated => format!(
            "The read request for {paths} was unrelated to the requested task {}.",
            explanation_quote(&task.excerpt)
        ),
        ReadRelationship::DistinctFileSetTooBroad => format!(
            "The assessed file set ({paths}) was broader than needed for the requested task {}.",
            explanation_quote(&task.excerpt)
        ),
        ReadRelationship::ReturnedRegionExcessive => {
            let result = selected_items.iter().find(|item| {
                reads
                    .iter()
                    .any(|read| read.result_reference.as_deref() == Some(item.reference.as_str()))
            })?;
            format!(
                "The selected passage {} from {paths} went beyond the requested task {}.",
                explanation_quote(&result.excerpt),
                explanation_quote(&task.excerpt)
            )
        }
        ReadRelationship::LaterReadRepeatsEarlier => {
            let equality = if basis.whole_output_equal == Some(true) {
                " The recorded outputs are identical."
            } else {
                ""
            };
            format!(
                "The later read of {paths} repeated an earlier read without helping the requested task {}.{equality}",
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
            ReadRelationship::DistinctFileSetTooBroad => "distinctFileSetTooBroad",
            ReadRelationship::ReturnedRegionExcessive => "returnedRegionExcessive",
            ReadRelationship::LaterReadRepeatsEarlier => "laterReadRepeatsEarlier",
        }
        .into(),
        text,
        references,
    })
}

#[cfg(test)]
mod tests {
    use super::selected_read_excerpt;

    #[test]
    fn selected_ranges_keep_the_saved_offset_for_repeated_text_and_reject_bad_byte_lengths() {
        let fields = serde_json::json!({
            "task": [{"range": [7, 11], "text": "same"}]
        });
        assert_eq!(
            selected_read_excerpt(&fields, "task[0]"),
            Some((7, 11, "same"))
        );

        let invalid_utf8_boundary = serde_json::json!({
            "task": [{"range": [1, 2], "text": "é"}]
        });
        assert_eq!(
            selected_read_excerpt(&invalid_utf8_boundary, "task[0]"),
            None
        );
    }
}
