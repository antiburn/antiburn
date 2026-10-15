use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::planning::{WorkGroup, digest};
use super::{ScopeExcerpt, questions};
use crate::analysis::jev::capabilities::ModelCapabilities;
use crate::analysis::jev::*;
use crate::analysis::jev_evidence::{ContentAction, JevOperationState, content_action_digest};
use crate::analysis::session_scope::{ScopeAuthority, SessionScopeSnapshot};

const TASK_BYTES: usize = 512;
const AUTHORITY_BYTES: usize = 512;
const WORK_BYTES: usize = 768;
const STATE_BYTES: usize = 2_048;
const REQUEST_BYTES: usize = 8_192;

pub(super) fn build_window(
    group: &mut WorkGroup,
    work: &[&ContentAction],
    actions: &[ContentAction],
    scope: &SessionScopeSnapshot,
    capabilities: &ModelCapabilities,
    selected: &BTreeSet<usize>,
    reduced: bool,
) -> Result<Vec<JevWorkItem>, JevError> {
    let Some(operation_action) = work.first().copied() else {
        group.limitation = Some("operation_unavailable".into());
        return Ok(Vec::new());
    };
    if work.len() != 2
        || work[1].kind != "tool_result"
        || work.iter().any(|action| action.truncated)
    {
        group.limitation = Some("operation_outcome_unbound".into());
        return Ok(Vec::new());
    }

    let anchor = &operation_action.reference;
    let active_task = scope
        .occurrences()
        .iter()
        .enumerate()
        .filter(|(_, occurrence)| {
            occurrence.authority == ScopeAuthority::User
                && occurrence.field == JevInputField::UserMessage
                && (
                    occurrence.reference.turn_index,
                    occurrence.reference.part_index,
                ) <= (anchor.turn_index, anchor.part_index)
        })
        .max_by_key(|(_, occurrence)| {
            (
                occurrence.reference.turn_index,
                occurrence.reference.part_index,
            )
        })
        .map(|(index, _)| index);
    let Some(task_index) = active_task else {
        group.limitation = Some("task_context_unavailable".into());
        return Ok(Vec::new());
    };
    if !selected.contains(&task_index)
        || super::planning::validate_scope_activity(scope, actions, selected).is_err()
    {
        group.limitation = Some("authority_history_incomplete".into());
        return Ok(Vec::new());
    }

    let task_occurrence = &scope.occurrences()[task_index];
    let task_action = actions
        .iter()
        .find(|action| action.reference == task_occurrence.reference)
        .ok_or(JevError::InvalidCheckContext)?;
    let task_text = scope.values()[task_occurrence.value_index]
        .as_str()
        .ok_or(JevError::InvalidCheckContext)?;
    if task_text.is_empty() || task_text.len() > TASK_BYTES || task_action.text != task_text {
        group.limitation = Some("task_context_too_large_or_unbound".into());
        return Ok(Vec::new());
    }

    let mut authority_indexes = BTreeSet::new();
    let mut pending_assistant = Vec::new();
    for (index, occurrence) in scope.occurrences().iter().enumerate() {
        if occurrence.authority == ScopeAuthority::User {
            if occurrence.field == JevInputField::UserMessage && index == task_index {
                authority_indexes.extend(pending_assistant.iter().copied());
                pending_assistant.clear();
                continue;
            }
            authority_indexes.extend(pending_assistant.drain(..));
            authority_indexes.insert(index);
        } else if occurrence.field == JevInputField::PlanReference
            && occurrence.authority == ScopeAuthority::SupportingContext
        {
            authority_indexes.insert(index);
        } else if occurrence.field == JevInputField::AssistantMessage {
            pending_assistant.push(index);
        }
    }
    if authority_indexes
        .iter()
        .any(|index| !selected.contains(index))
    {
        group.limitation = Some("authority_history_incomplete".into());
        return Ok(Vec::new());
    }
    let mut authority_bytes = 0usize;
    let mut evidence = Vec::new();
    let mut excerpts = Vec::new();
    let mut authority_records = Vec::new();
    for (ordinal, index) in authority_indexes.iter().copied().enumerate() {
        let occurrence = &scope.occurrences()[index];
        let source = actions
            .iter()
            .find(|action| action.reference == occurrence.reference)
            .ok_or(JevError::InvalidCheckContext)?;
        let value = &scope.values()[occurrence.value_index];
        let (text, range_source) = if let Some(text) = value.as_str()
            && source.text == text
        {
            (text.to_owned(), "selected_action_text")
        } else if matches!(
            occurrence.field,
            JevInputField::UserAnswer | JevInputField::PlanReference
        ) {
            let mut value = value.clone();
            value
                .as_object_mut()
                .ok_or(JevError::InvalidCheckContext)?
                .remove("source");
            (
                serde_json::to_string(&value).map_err(|_| JevError::InvalidCheckContext)?,
                "scope_value_json",
            )
        } else {
            group.limitation = Some("authority_binding_invalid".into());
            return Ok(Vec::new());
        };
        authority_bytes = authority_bytes.saturating_add(text.len());
        if authority_bytes > AUTHORITY_BYTES {
            group.limitation = Some("authority_history_too_large".into());
            return Ok(Vec::new());
        }
        let part_id = format!("authorization[{ordinal}]");
        evidence.push(JevEvidenceReference {
            part_id: part_id.clone(),
            source_id: source.reference.id.clone(),
            content_kind: format!("{:?}", occurrence.field),
            role: if occurrence.authority == ScopeAuthority::User {
                JevEvidenceRole::Instruction
            } else {
                JevEvidenceRole::SupportingContext
            },
        });
        authority_records.push(json!({
            "authority": if occurrence.authority == ScopeAuthority::User {
                "user"
            } else if occurrence.field == JevInputField::PlanReference
                && occurrence.authority == ScopeAuthority::SupportingContext
                && value["origin"] == "user"
                && value["status"] == "approved"
            {
                "user_approved_plan"
            } else {
                "non_authorizing"
            },
            "turn": occurrence.reference.turn_index,
            "field": occurrence.field,
            "text": text,
        }));
        excerpts.push(scope_excerpt(source, &text, occurrence, range_source));
    }
    if serde_json::to_vec(&authority_records)
        .map_err(|_| JevError::InvalidCheckContext)?
        .len()
        > AUTHORITY_BYTES
    {
        group.limitation = Some("authority_history_too_large".into());
        return Ok(Vec::new());
    }

    let task_part = "task".to_owned();
    evidence.push(JevEvidenceReference {
        part_id: task_part.clone(),
        source_id: task_action.reference.id.clone(),
        content_kind: format!("{:?}", task_occurrence.field),
        role: JevEvidenceRole::Instruction,
    });
    excerpts.push(scope_excerpt(
        task_action,
        task_text,
        task_occurrence,
        "selected_action_text",
    ));

    let Some(operation) = native_operation(operation_action) else {
        group.limitation = Some("operation_binding_ambiguous_or_unsupported".into());
        return Ok(Vec::new());
    };
    let unresolved_state = operation_action.metadata.state == JevOperationState::Error
        || matches!(
            work[1].metadata.state,
            JevOperationState::Error | JevOperationState::Pending | JevOperationState::Running
        );
    if unresolved_state {
        group.limitation = Some("operation_not_completed".into());
        return Ok(Vec::new());
    }
    let outcome = work[1].text.as_str();
    if outcome.trim().is_empty() || outcome.len() > WORK_BYTES {
        group.limitation = Some("operation_outcome_too_large_or_missing".into());
        return Ok(Vec::new());
    }
    let work_value = json!({ "operation": operation, "outcome": outcome, "recorded_state": work[1].metadata.state });
    if serde_json::to_vec(&work_value)
        .map_err(|_| JevError::InvalidCheckContext)?
        .len()
        > WORK_BYTES
    {
        group.limitation = Some("operation_evidence_too_large".into());
        return Ok(Vec::new());
    }
    if operation_action.text.chars().count() > 4_096 {
        group.limitation = Some("operation_source_too_large".into());
        return Ok(Vec::new());
    }
    for (part_id, source, role) in [
        ("operation", operation_action, JevEvidenceRole::Candidate),
        ("outcome", work[1], JevEvidenceRole::SupportingContext),
    ] {
        evidence.push(JevEvidenceReference {
            part_id: part_id.into(),
            source_id: source.reference.id.clone(),
            content_kind: source.kind.clone(),
            role,
        });
    }
    excerpts.push(activity_excerpt(operation_action));
    excerpts.push(activity_excerpt(work[1]));

    let fields = json!({
        "task": task_text,
        "authorization": authority_records,
        "authority_complete": true,
        "limitations": Vec::<String>::new(),
        "work": work_value,
    });
    let mut item = JevWorkItem {
        id: digest(&(&group.id, &fields, &evidence))?,
        window: JevInputWindow { fields, evidence },
        questions: questions::questions(),
    };
    if reduced {
        item.window
            .fields
            .as_object_mut()
            .ok_or(JevError::InvalidCheckContext)?
            .remove("limitations");
        item.window.fields["work"]
            .as_object_mut()
            .ok_or(JevError::InvalidCheckContext)?
            .remove("recorded_state");
        if !request_fits(std::slice::from_ref(&item), capabilities) {
            group.limitation = Some("work_context_too_large".into());
            return Ok(Vec::new());
        }
        item.id = digest(&(&group.id, &item.window.fields, &item.window.evidence))?;
    }
    group.task_scope = item
        .window
        .evidence
        .iter()
        .filter(|reference| {
            reference.part_id == task_part || reference.part_id.starts_with("authorization[")
        })
        .enumerate()
        .map(|(index, reference)| JevEvidenceReference {
            part_id: format!("task_scope[{index}]"),
            ..reference.clone()
        })
        .collect();
    group.selected_excerpts = excerpts;
    group.window_ids = vec![item.id.clone()];
    Ok(vec![item])
}

pub(super) fn request_fits(items: &[JevWorkItem], capabilities: &ModelCapabilities) -> bool {
    if items
        .iter()
        .any(|item| item.window.fields.to_string().len() > STATE_BYTES)
    {
        return false;
    }
    crate::analysis::jev::work_items_fit_one_request(
        items,
        capabilities,
        None,
        crate::analysis::jev::JevRequestLimits {
            request_bytes: REQUEST_BYTES,
            estimated_input_tokens: 2048,
        },
    )
    .unwrap_or(false)
}

fn native_operation(action: &ContentAction) -> Option<Value> {
    let normalized = action.normalized_fields.as_ref()?;
    if normalized.malformed {
        return None;
    }
    let values = &normalized.values;
    match normalized.category? {
        JevNormalizedCategory::FileEdit => {
            let path_value: Value =
                serde_json::from_str(values.get(&JevInputField::FileEditPath)?).ok()?;
            let content = values.get(&JevInputField::FileEditContent)?;
            let mut content_value: Value =
                serde_json::from_str(content).unwrap_or_else(|_| json!(content));
            let paths = path_value.get("paths")?.as_array()?;
            if paths.len() != 1 {
                return None;
            }
            if let Some(changes) = content_value.get("changes").and_then(Value::as_object) {
                let path = paths[0].as_str()?;
                if changes.len() != 1 {
                    return None;
                }
                let change = changes.get(path)?;
                content_value =
                    if let Some(diff) = change.get("unified_diff").and_then(Value::as_str) {
                        json!({"patch_content": compact_bound_patch(diff, path)?})
                    } else {
                        let content = change.get("content").and_then(Value::as_str)?;
                        json!({"new_content": content})
                    };
            }
            let source = content_value
                .get("patch_content")
                .and_then(Value::as_str)
                .unwrap_or(content);
            if content_value.get("patch_content").is_some()
                || source.contains("*** Update File: ")
                || source.contains("*** Add File: ")
                || source.contains("*** Delete File: ")
                || source.starts_with("diff --git ")
            {
                let bound_path = paths[0].as_str()?;
                let patch = compact_bound_patch(source, bound_path)?;
                if content_value.get("patch_content").is_some() {
                    content_value["patch_content"] = json!(patch);
                } else {
                    content_value = json!({"patch": patch});
                }
                if serde_json::to_vec(&content_value).ok()?.len() > WORK_BYTES {
                    return None;
                }
            }
            Some(json!({"kind":"file_edit","path":paths[0],"change":content_value}))
        }
        JevNormalizedCategory::BashCommand => {
            let command = values.get(&JevInputField::BashCommandInput)?;
            (command.len() <= 640).then(|| json!({"kind":"command","command":command}))
        }
        _ => None,
    }
}

fn compact_bound_patch(source: &str, bound_path: &str) -> Option<String> {
    use crate::analysis::jev::edit_hunks::{EditLineKind, EditPathOperation, edit_hunks};

    let apply_operations = crate::analysis::jev::edit_hunks::apply_patch_path_operations(source);
    if !apply_operations.is_empty() {
        if apply_operations.len() != 1 {
            return None;
        }
        let operation_path = match &apply_operations[0] {
            EditPathOperation::Add { path }
            | EditPathOperation::Update { path }
            | EditPathOperation::Delete { path } => path,
            EditPathOperation::Move { to, .. } => to,
        };
        if operation_path != bound_path {
            return None;
        }
        if source.len() <= 640 {
            return Some(source.to_owned());
        }
        let hunks = edit_hunks(source);
        let hunks: Vec<_> = hunks
            .into_iter()
            .filter(|hunk| {
                hunk.lines
                    .iter()
                    .any(|line| matches!(line.kind, EditLineKind::Added | EditLineKind::Removed))
            })
            .collect();
        if hunks.len() != 1 {
            return None;
        }
        return hunks.into_iter().find_map(|hunk| {
            let header = source.get(hunk.file_header?)?;
            let header_path = header
                .strip_prefix("*** Update File: ")
                .or_else(|| header.strip_prefix("*** Add File: "))
                .or_else(|| header.strip_prefix("*** Delete File: "))?
                .trim_end_matches(['\r', '\n']);
            (header_path == bound_path
                && hunk.range.len() <= 640
                && hunk
                    .lines
                    .iter()
                    .any(|line| matches!(line.kind, EditLineKind::Added | EditLineKind::Removed)))
            .then(|| source[hunk.range].to_owned())
        });
    }

    let hunks = edit_hunks(source);
    if hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == EditLineKind::Metadata)
        .filter_map(|line| source.get(line.range.clone()))
        .any(|line| {
            line.starts_with("+++ ")
                && line.trim_end() != format!("+++ b/{bound_path}")
                && line.trim_end() != "+++ /dev/null"
        })
    {
        return None;
    }
    let headers: BTreeSet<_> = hunks
        .iter()
        .filter_map(|hunk| hunk.file_header.as_ref())
        .filter_map(|range| source.get(range.clone()))
        .collect();
    if headers.iter().any(|header| {
        if header.starts_with("diff --git ") {
            let fields: Vec<_> = header.split_whitespace().collect();
            let ["diff", "--git", old, new] = fields.as_slice() else {
                return true;
            };
            old.strip_prefix("a/") != Some(bound_path) || new.strip_prefix("b/") != Some(bound_path)
        } else {
            header.trim_end() != format!("--- a/{bound_path}")
                && header.trim_end() != "--- /dev/null"
        }
    }) {
        return None;
    }
    let hunks: Vec<_> = hunks
        .into_iter()
        .filter(|hunk| {
            hunk.lines
                .iter()
                .any(|line| matches!(line.kind, EditLineKind::Added | EditLineKind::Removed))
        })
        .collect();
    if hunks.is_empty() || (source.len() > 640 && hunks.len() != 1) {
        return None;
    }
    if source.len() <= 640 {
        return Some(source.to_owned());
    }
    hunks.into_iter().find_map(|hunk| {
        (hunk.range.len() <= 640
            && hunk
                .lines
                .iter()
                .any(|line| matches!(line.kind, EditLineKind::Added | EditLineKind::Removed)))
        .then(|| source[hunk.range].to_owned())
    })
}

fn scope_excerpt(
    action: &ContentAction,
    text: &str,
    occurrence: &crate::analysis::session_scope::ScopeOccurrence,
    range_source: &str,
) -> ScopeExcerpt {
    ScopeExcerpt {
        source: action.reference.clone(),
        content_digest: content_action_digest(action),
        start_byte: 0,
        end_byte: text.len(),
        text: text.to_owned(),
        authority: match occurrence.authority {
            ScopeAuthority::User => "user",
            ScopeAuthority::SupportingContext => "supporting_context",
            _ => "non_authorizing",
        }
        .into(),
        kind: action.kind.clone(),
        range_source: range_source.into(),
        scope_field: Some(occurrence.field),
        native_source: occurrence.native_source.clone(),
    }
}

fn activity_excerpt(action: &ContentAction) -> ScopeExcerpt {
    ScopeExcerpt {
        source: action.reference.clone(),
        content_digest: content_action_digest(action),
        start_byte: 0,
        end_byte: action.text.len(),
        text: action.text.clone(),
        authority: action.authority.clone(),
        kind: action.kind.clone(),
        range_source: "selected_action_text".into(),
        scope_field: None,
        native_source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_hunks_keep_exact_file_bindings_and_do_not_sample_multiple_changes() {
        let patch = "diff --git a/src/parser.rs b/src/parser.rs\n--- a/src/parser.rs\n+++ b/src/parser.rs\n@@ -1 +1 @@\n-if !enabled { return; }\n+if enabled { return; }\n";
        assert_eq!(
            compact_bound_patch(patch, "src/parser.rs").as_deref(),
            Some(patch)
        );
        assert!(compact_bound_patch(patch, "src/billing.rs").is_none());
        let large = format!(
            "{patch}@@ -5 +5 @@\n-old\n+{}\n",
            "later condition ".repeat(64)
        );
        assert!(compact_bound_patch(&large, "src/parser.rs").is_none());
        let multi = "*** Begin Patch\n*** Update File: src/parser.rs\n@@\n-before\n+after\n*** Update File: src/billing.rs\n@@\n-old\n+new\n*** End Patch";
        assert!(compact_bound_patch(multi, "src/parser.rs").is_none());
    }
}
