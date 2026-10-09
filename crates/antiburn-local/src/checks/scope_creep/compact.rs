use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::planning::{WorkGroup, digest};
use super::{ScopeExcerpt, questions};
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::*;
use crate::analysis::jev_evidence::ContentAction;
use crate::analysis::session_scope::SessionScopeSnapshot;

pub(super) fn build_window(
    group: &mut WorkGroup,
    work: &[&ContentAction],
    actions: &[ContentAction],
    scope: &SessionScopeSnapshot,
    capabilities: &ModelCapabilities,
    selected: &BTreeSet<usize>,
) -> Result<Vec<JevWorkItem>, JevError> {
    let anchor = &work[0].reference;
    let position = (anchor.turn_index, anchor.part_index);
    let mut records = selected.iter().copied().collect::<Vec<_>>();
    records.sort_by_key(|index| {
        let record = &scope.occurrences()[*index];
        (
            record.reference.turn_index.abs_diff(anchor.turn_index),
            record.reference.part_index,
        )
    });
    // Keep a short approval and its proposal together.
    let mut retained = BTreeSet::new();
    let mut linked_proposals = BTreeSet::new();
    for index in records.into_iter().take(4) {
        retained.insert(index);
        if scope.occurrences()[index].authority
            == crate::analysis::session_scope::ScopeAuthority::User
            && scope.values()[scope.occurrences()[index].value_index]
                .as_str()
                .is_some_and(|text| text.len() < 128)
            && let Some(previous) = scope.occurrences()[..index]
                .iter()
                .enumerate()
                .rev()
                .take_while(|(_, record)| {
                    record.authority != crate::analysis::session_scope::ScopeAuthority::User
                })
                .find(|(_, record)| record.field == JevInputField::AssistantMessage)
                .map(|(index, _)| index)
        {
            retained.insert(previous);
            linked_proposals.insert(scope.occurrences()[previous].reference.id.clone());
        }
    }
    retained.retain(|index| {
        !work
            .iter()
            .any(|action| action.reference == scope.occurrences()[*index].reference)
    });
    if !retained.iter().any(|index| {
        let record = &scope.occurrences()[*index];
        record.authority == crate::analysis::session_scope::ScopeAuthority::User
            && (record.reference.turn_index, record.reference.part_index) <= position
    }) {
        group.limitation = Some("task_context_unavailable".into());
        return Ok(Vec::new());
    }
    if super::planning::validate_scope_activity(scope, actions, &retained).is_err() {
        group.limitation = Some("selected_scope_dependency_invalid".into());
        return Ok(Vec::new());
    }
    let mut task = Vec::new();
    for index in retained.iter().copied() {
        let record = &scope.occurrences()[index];
        let Some(action) = actions
            .iter()
            .find(|action| action.reference == record.reference)
        else {
            group.limitation = Some("selected_scope_dependency_invalid".into());
            return Ok(Vec::new());
        };
        let value = &scope.values()[record.value_index];
        let text = if let Some(text) = value.as_str() {
            if action.text != text || !matches!(action.authority.as_str(), "user" | "assistant") {
                group.limitation = Some("selected_scope_dependency_invalid".into());
                return Ok(Vec::new());
            }
            text.to_owned()
        } else {
            serde_json::to_string(value).map_err(|_| JevError::InvalidCheckContext)?
        };
        task.push((action, text, record));
    }
    let terms = work[0]
        .text
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() >= 5)
        .take(32)
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    for bytes in [512, 256, 128, 64] {
        let mut excerpts = Vec::new();
        let mut evidence = Vec::new();
        let mut partial = retained.len() < selected.len() || !scope.limitations().is_empty();
        let mut render = |action: &ContentAction,
                          text: &str,
                          key: String,
                          role: JevEvidenceRole,
                          authority: &str,
                          scope_record: Option<
            &crate::analysis::session_scope::ScopeOccurrence,
        >| {
            let ranges = if linked_proposals.contains(&action.reference.id)
                || scope_record.is_some_and(|record| {
                    !matches!(
                        record.field,
                        JevInputField::UserMessage | JevInputField::AssistantMessage
                    )
                }) {
                vec![(0, text.len())]
            } else {
                semantic_ranges(text, bytes, &terms)
            };
            partial |= ranges != [(0, text.len())] || action.truncated;
            evidence.push(JevEvidenceReference {
                part_id: key,
                source_id: action.reference.id.clone(),
                content_kind: action.kind.clone(),
                role,
            });
            let chunks = ranges
                .into_iter()
                .map(|(start, end)| {
                    excerpts.push(ScopeExcerpt {
                        source: action.reference.clone(),
                        content_digest: crate::analysis::jev_evidence::content_action_digest(
                            action,
                        ),
                        start_byte: start,
                        end_byte: end,
                        text: text[start..end].to_owned(),
                        authority: authority.to_owned(),
                        kind: action.kind.clone(),
                        scope_field: scope_record.map(|record| record.field),
                        native_source: scope_record.and_then(|record| record.native_source.clone()),
                        range_source: if text == action.text {
                            "selected_action_text"
                        } else {
                            "scope_value_json"
                        }
                        .into(),
                    });
                    text[start..end].to_owned()
                })
                .collect::<Vec<_>>();
            json!({"authority": authority, "kind": action.kind, "turn": action.reference.turn_index, "text": chunks})
        };
        let task_scope = task
            .iter()
            .enumerate()
            .map(|(index, (action, text, record))| {
                render(
                    action,
                    text,
                    format!("task_scope[{index}]"),
                    JevEvidenceRole::SupportingContext,
                    match record.authority {
                        crate::analysis::session_scope::ScopeAuthority::User => "user",
                        crate::analysis::session_scope::ScopeAuthority::SupportingContext => {
                            "supporting_context"
                        }
                        _ => "non_authorizing",
                    },
                    Some(*record),
                )
            })
            .collect::<Vec<_>>();
        let bound_work = work
            .iter()
            .enumerate()
            .map(|(index, action)| {
                render(
                    action,
                    &action.text,
                    format!("bound_work[{index}]"),
                    JevEvidenceRole::Candidate,
                    &action.authority,
                    None,
                )
            })
            .collect::<Vec<_>>();
        let fields = json!({"task_scope": task_scope, "bound_work": bound_work, "observation": group.observation_kind, "partial": partial});
        let item = JevWorkItem {
            id: digest(&(&group.id, &fields, &evidence))?,
            window: JevInputWindow { fields, evidence },
            questions: questions::questions(),
        };
        if !pack_work_items_with_capabilities(std::slice::from_ref(&item), capabilities)
            .batches
            .is_empty()
        {
            group.task_scope = item
                .window
                .evidence
                .iter()
                .filter(|reference| reference.part_id.starts_with("task_scope["))
                .cloned()
                .collect();
            group.selected_excerpts = excerpts;
            group.window_ids = vec![item.id.clone()];
            if partial {
                group.limitation = Some("scope_window_partial".into());
            }
            return Ok(vec![item]);
        }
    }
    group.limitation = Some("work_context_too_large".into());
    Ok(Vec::new())
}

pub(super) fn legacy_excerpts(
    fields: &Value,
    work: &[&ContentAction],
    context: &[&ContentAction],
    actions: &[ContentAction],
    scope: &SessionScopeSnapshot,
    selected: &BTreeSet<usize>,
) -> Vec<ScopeExcerpt> {
    let mut excerpts = Vec::new();
    let mut append =
        |action: &ContentAction,
         content: &Value,
         scope_record: Option<&crate::analysis::session_scope::ScopeOccurrence>| {
            let (ranges, range_source) = if let Some(chunks) = content["chunks"].as_array() {
                (
                    chunks
                        .iter()
                        .map(|chunk| {
                            (
                                chunk["start_byte"].as_u64().expect("start byte") as usize,
                                chunk["end_byte"].as_u64().expect("end byte") as usize,
                                chunk["text"].as_str().expect("excerpt").to_owned(),
                            )
                        })
                        .collect::<Vec<_>>(),
                    content["range_source"]
                        .as_str()
                        .unwrap_or("selected_action_text"),
                )
            } else {
                let text = content
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| content.to_string());
                let bytes = text.len();
                (
                    vec![(0, bytes, text)],
                    if content.is_string() {
                        "selected_action_text"
                    } else {
                        "scope_value_json"
                    },
                )
            };
            excerpts.extend(
                ranges
                    .into_iter()
                    .map(|(start_byte, end_byte, text)| ScopeExcerpt {
                        source: action.reference.clone(),
                        content_digest: crate::analysis::jev_evidence::content_action_digest(
                            action,
                        ),
                        start_byte,
                        end_byte,
                        text,
                        authority: action.authority.clone(),
                        kind: action.kind.clone(),
                        range_source: range_source.into(),
                        scope_field: scope_record.map(|record| record.field),
                        native_source: scope_record.and_then(|record| record.native_source.clone()),
                    }),
            );
        };
    for (index, action) in work.iter().enumerate() {
        if action.kind == "tool_result" {
            continue;
        }
        append(action, &fields["bound_work"][index]["content"], None);
    }
    for (index, action) in context.iter().enumerate() {
        append(
            action,
            &fields["supporting_activity"][index]["content"],
            None,
        );
    }
    for (index, selected) in selected.iter().enumerate() {
        let record = &scope.occurrences()[*selected];
        let action = actions
            .iter()
            .find(|action| action.reference == record.reference)
            .expect("validated scope action");
        append(
            action,
            &fields["task_scope"][index]["content"],
            Some(record),
        );
    }
    excerpts
}

fn semantic_ranges(text: &str, bytes: usize, terms: &[String]) -> Vec<(usize, usize)> {
    if text.len() <= bytes {
        return vec![(0, text.len())];
    }
    let mut units = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        let end = index + character.len_utf8();
        if character != '\n'
            && !(character == '.' && text[end..].chars().next().is_none_or(char::is_whitespace))
        {
            continue;
        }
        if end - start <= bytes {
            units.push((start, end));
        }
        start = end;
    }
    if start < text.len() && text.len() - start <= bytes {
        units.push((start, text.len()));
    }
    if let Some(range) = units.into_iter().max_by_key(|(start, end)| {
        let snippet = text[*start..*end].to_lowercase();
        terms
            .iter()
            .filter(|term| snippet.contains(term.as_str()))
            .count()
    }) {
        return vec![range];
    }
    crate::analysis::jev::text_ranges::text_ranges(text, bytes, 0)
        .into_iter()
        .take(1)
        .collect()
}
