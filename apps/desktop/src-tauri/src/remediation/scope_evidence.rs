use super::models::*;
use super::{bounded_evidence_excerpt, explanation_quote};
use antiburn_local::analysis::jev::{JevEvidenceRole, JevInputField};
use antiburn_local::analysis::jev_evidence::{
    ContentAction, ContentEventReference, content_action_digest,
};
use antiburn_local::analysis::session_scope::{
    ScopeAuthority, ScopeOccurrence, SessionScopeSnapshot,
};
use antiburn_local::checks::scope_creep::{
    ScopeCreepFinding, ScopeExcerpt, WorkBinding, WorkGroup, WorkObservationKind,
};
use std::collections::BTreeMap;

pub(super) fn accepted_scope_group(
    publication: &crate::scope_creep_worker::Publication,
    finding: &ScopeCreepFinding,
) -> Option<WorkGroup> {
    let saved = serde_json::to_value(publication).ok()?;
    let inventory = saved.get("prepared")?.get("groups")?;
    let groups = inventory.get("groups")?.as_array()?;
    let group = groups.iter().find(|group| {
        group.get(0).and_then(serde_json::Value::as_str) == Some(finding.group_id.as_str())
    })?;
    let bindings = inventory.get("bindings")?.as_array()?;
    let sources = inventory.get("sources")?.as_array()?;
    let expand = |indices: &serde_json::Value| -> Option<Vec<WorkBinding>> {
        indices
            .as_array()?
            .iter()
            .map(|index| {
                let binding = bindings.get(usize::try_from(index.as_u64()?).ok()?)?;
                let (id, key, thread, turn_index, native_record_id, part_index, stable, digest): (
                    String,
                    usize,
                    usize,
                    u64,
                    Option<String>,
                    u32,
                    bool,
                    String,
                ) = serde_json::from_value(binding.clone()).ok()?;
                Some(WorkBinding {
                    reference: ContentEventReference {
                        id,
                        source_key_digest: sources.get(key)?.as_str()?.into(),
                        thread_digest: sources.get(thread)?.as_str()?.into(),
                        turn_index,
                        native_record_id,
                        part_index,
                        stable,
                    },
                    digest,
                })
            })
            .collect()
    };
    Some(WorkGroup {
        id: finding.group_id.clone(),
        semantic_digest: serde_json::from_value(group.get(1)?.clone()).ok()?,
        work: expand(group.get(2)?)?,
        context: expand(group.get(3)?)?,
        window_ids: serde_json::from_value(group.get(4)?.clone()).ok()?,
        limitation: serde_json::from_value(group.get(5)?.clone()).ok()?,
        task_scope: serde_json::from_value(group.get(6)?.clone()).ok()?,
        observation_kind: serde_json::from_value(group.get(7)?.clone()).ok()?,
        selected_excerpts: serde_json::from_value(group.get(8)?.clone()).ok()?,
    })
}

fn scope_value(
    record: &ScopeOccurrence,
    scope: &SessionScopeSnapshot,
    action: &ContentAction,
) -> Option<serde_json::Value> {
    let expected = scope.values().get(record.value_index)?;
    let mut actual = match record.field {
        JevInputField::UserMessage if action.authority == "user" && action.turn_role == "user" => {
            serde_json::json!(action.text)
        }
        JevInputField::AssistantMessage
            if action.authority == "assistant" && action.turn_role == "assistant" =>
        {
            serde_json::json!(action.text)
        }
        JevInputField::UserAnswer => serde_json::to_value(
            action
                .metadata
                .user_answers
                .iter()
                .find(|answer| Some(&answer.source) == record.native_source.as_ref())?,
        )
        .ok()?,
        JevInputField::PlanReference => serde_json::to_value(
            action
                .metadata
                .plan_references
                .iter()
                .find(|plan| Some(&plan.source) == record.native_source.as_ref())?,
        )
        .ok()?,
        _ => return None,
    };
    if let Some(object) = actual.as_object_mut() {
        object.remove("source");
    }
    (actual == *expected).then_some(actual)
}

fn scope_authority(authority: ScopeAuthority) -> &'static str {
    match authority {
        ScopeAuthority::User => "user",
        ScopeAuthority::SupportingContext => "supporting_context",
        _ => "non_authorizing",
    }
}

fn validate_excerpt(
    excerpt: &ScopeExcerpt,
    scope: &SessionScopeSnapshot,
    actions: &[ContentAction],
) -> Option<()> {
    let action = actions
        .iter()
        .find(|action| action.reference == excerpt.source)?;
    if !action.reference.stable
        || action.kind != excerpt.kind
        || content_action_digest(action) != excerpt.content_digest
        || excerpt.start_byte >= excerpt.end_byte
    {
        return None;
    }
    let text = match excerpt.scope_field {
        Some(field) => {
            let record = scope.occurrences().iter().find(|record| {
                record.reference == excerpt.source
                    && record.field == field
                    && record.native_source == excerpt.native_source
            })?;
            if scope_authority(record.authority) != excerpt.authority {
                return None;
            }
            let value = scope_value(record, scope, action)?;
            match excerpt.range_source.as_str() {
                "selected_action_text" if value.as_str() == Some(action.text.as_str()) => {
                    action.text.clone()
                }
                "scope_value_json"
                    if matches!(
                        field,
                        JevInputField::UserAnswer | JevInputField::PlanReference
                    ) =>
                {
                    serde_json::to_string(&value).ok()?
                }
                _ => return None,
            }
        }
        None if excerpt.native_source.is_none()
            && excerpt.range_source == "selected_action_text"
            && excerpt.authority == action.authority =>
        {
            action.text.clone()
        }
        _ => return None,
    };
    (text.get(excerpt.start_byte..excerpt.end_byte) == Some(excerpt.text.as_str())).then_some(())
}

pub(super) fn scope_group_evidence(
    finding: &ScopeCreepFinding,
    group: &WorkGroup,
    scope: &SessionScopeSnapshot,
    actions: &[ContentAction],
) -> Option<BurnCheckTargetEvidence> {
    if group.id != finding.group_id
        || group.work != finding.work
        || group.task_scope != finding.task_scope
        || group.observation_kind != finding.observation_kind
        || finding.source_generation != scope.source_generation()
        || finding.publication_fence != scope.publication_fence()
    {
        return None;
    }
    let mut items = BTreeMap::new();
    for binding in group.work.iter().chain(&group.context) {
        let action = actions
            .iter()
            .find(|action| action.reference == binding.reference)?;
        if !binding.reference.stable || content_action_digest(action) != binding.digest {
            return None;
        }
        let work = group.work.contains(binding);
        items.insert(
            binding.reference.id.clone(),
            BurnCheckEvidenceItem {
                label: if work {
                    BurnCheckEvidenceLabel::ObservedAction
                } else {
                    BurnCheckEvidenceLabel::Context
                },
                source_label: if work {
                    "Recorded work"
                } else {
                    "Supporting activity"
                }
                .into(),
                reference: binding.reference.id.clone(),
                observed_at_ms: action.timestamp_ms,
                start_line: None,
                end_line: None,
                excerpt: bounded_evidence_excerpt(&action.text),
                explanation: String::new(),
                limitation: None,
            },
        );
    }
    for (binding_index, binding) in group.task_scope.iter().enumerate() {
        let record = scope
            .occurrences()
            .iter()
            .find(|record| record.reference.id == binding.source_id)?;
        let action = actions
            .iter()
            .find(|action| action.reference == record.reference)?;
        let role_matches = match (record.field, record.authority) {
            (JevInputField::UserMessage, ScopeAuthority::User) => {
                binding.role == JevEvidenceRole::Instruction
            }
            _ => binding.role == JevEvidenceRole::SupportingContext,
        };
        if binding.part_id != format!("task_scope[{binding_index}]")
            || binding.content_kind != format!("{:?}", record.field)
            || !role_matches
            || !group.selected_excerpts.iter().any(|excerpt| {
                excerpt.source == record.reference
                    && excerpt.scope_field == Some(record.field)
                    && excerpt.native_source == record.native_source
            })
        {
            return None;
        }
        let value = scope_value(record, scope, action)?;
        let objective =
            record.field == JevInputField::UserMessage && record.authority == ScopeAuthority::User;
        let label = match record.field {
            JevInputField::UserMessage if objective => "Requested task",
            JevInputField::UserAnswer => "Recorded user answer",
            JevInputField::PlanReference => "Recorded plan",
            JevInputField::AssistantMessage => "Recorded proposal",
            _ => "Recorded task context",
        };
        let text = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        items
            .entry(binding.source_id.clone())
            .or_insert(BurnCheckEvidenceItem {
                label: if objective {
                    BurnCheckEvidenceLabel::Instruction
                } else {
                    BurnCheckEvidenceLabel::Context
                },
                source_label: label.into(),
                reference: binding.source_id.clone(),
                observed_at_ms: action.timestamp_ms,
                start_line: None,
                end_line: None,
                excerpt: bounded_evidence_excerpt(&text),
                explanation: String::new(),
                limitation: None,
            });
        if objective {
            let item = items.get_mut(&binding.source_id)?;
            item.label = BurnCheckEvidenceLabel::Instruction;
            item.source_label = "Requested task".into();
            item.excerpt = bounded_evidence_excerpt(&text);
        }
    }
    let mut comparison = BurnCheckEvidenceComparison {
        observation_kind: Some(match finding.observation_kind {
            WorkObservationKind::Attempt => BurnCheckObservationKind::Attempt,
            WorkObservationKind::Proposal => BurnCheckObservationKind::Proposal,
        }),
        ..Default::default()
    };
    if finding.explanation_basis.is_none() && !items.is_empty() {
        let activity = match finding.observation_kind {
            WorkObservationKind::Attempt => "attempt",
            WorkObservationKind::Proposal => "proposal",
        };
        comparison.explanation = Some(BurnCheckExplanation {
            version: 1,
            relationship: "recordedEvidence".into(),
            text: format!(
                "The finding uses recorded {activity} activity and retained task context. Review the evidence before acting."
            ),
            references: Vec::new(),
        });
    }
    if let Some(basis) = &finding.explanation_basis {
        if basis.schema_revision != 1
            || basis.observation_kind != group.observation_kind
            || basis.excerpts != group.selected_excerpts
            || basis.excerpts.is_empty()
        {
            return None;
        }
        for excerpt in &basis.excerpts {
            validate_excerpt(excerpt, scope, actions)?;
            if !items.contains_key(&excerpt.source.id) {
                return None;
            }
        }
        for item in items.values_mut() {
            let passages = basis
                .excerpts
                .iter()
                .filter(|excerpt| excerpt.source.id == item.reference)
                .map(|excerpt| excerpt.text.as_str())
                .collect::<Vec<_>>();
            if !passages.is_empty() {
                item.excerpt = passages.join("\n…\n");
            }
        }
        let task_excerpt = basis.excerpts.iter().find(|excerpt| {
            excerpt.scope_field == Some(JevInputField::UserMessage)
                && excerpt.authority == "user"
                && scope.occurrences().iter().any(|record| {
                    record.reference == excerpt.source
                        && record.field == JevInputField::UserMessage
                        && record.authority == ScopeAuthority::User
                })
        })?;
        let task = items.get(&task_excerpt.source.id)?;
        if task.label != BurnCheckEvidenceLabel::Instruction {
            return None;
        }
        let work = items
            .values()
            .find(|item| item.label == BurnCheckEvidenceLabel::ObservedAction)?;
        let verb = match finding.observation_kind {
            WorkObservationKind::Attempt => "attempted",
            WorkObservationKind::Proposal => "proposed",
        };
        comparison.explanation = Some(BurnCheckExplanation {
            version: 1,
            relationship: "separateObjective".into(),
            text: format!(
                "The requested task was {}. The agent {verb} {}, a separate objective from that task.",
                explanation_quote(&task.excerpt),
                explanation_quote(&work.excerpt)
            ),
            references: basis
                .excerpts
                .iter()
                .map(|excerpt| excerpt.source.id.clone())
                .collect(),
        });
        comparison.source_ranges = basis
            .excerpts
            .iter()
            .map(|excerpt| BurnCheckSourceRange {
                reference: excerpt.source.id.clone(),
                start_byte: excerpt.start_byte,
                end_byte: excerpt.end_byte,
                range_source: excerpt.range_source.clone(),
            })
            .collect();
    }
    Some(BurnCheckTargetEvidence {
        comparison: Some(comparison),
        decision_proof: None,
        status: BurnCheckEvidenceStatus::Available,
        items: items.into_values().collect(),
        occurrences: Vec::new(),
    })
}
