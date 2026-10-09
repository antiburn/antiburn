use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::analysis::jev::{
    JevError, JevEvidenceReference, JevEvidenceRole, JevInputField, JevSessionContext,
    JevSharedRequestContext,
};
use crate::analysis::jev_evidence::{
    ContentAction, JevReadRequest, JevReadResult, SessionContentEvidence,
};
use crate::analysis::session_scope::{ScopeAuthority, SessionScopeSnapshot};
use crate::checks::ignored_instructions::sha256_hex;
use crate::checks::sampling::StableId;

pub(super) const MAX_EPISODES: usize = 256;
pub(super) const MAX_EVENTS: usize = 4096;
const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeState {
    Complete,
    Deferred,
}

/// The source boundary supplies completion. Idle alone does not prove that an
/// investigation ends. Include the recorded goal and all investigation events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeSpan {
    pub first_event_id: String,
    pub last_event_id: String,
    pub state: EpisodeState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadObservation {
    pub request: JevReadRequest,
    pub result: Option<JevReadResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationEpisode {
    pub id: StableId,
    pub state: EpisodeState,
    pub before: Vec<usize>,
    pub events: Vec<usize>,
    /// All later recorded work through the same assessment boundary. No edit
    /// is required for a read to be useful.
    pub subsequent: Vec<usize>,
    pub reads: Vec<ReadObservation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverExploringInput {
    pub session_identity: String,
    pub task_context: JevSharedRequestContext,
    pub task_contexts: BTreeMap<StableId, JevSharedRequestContext>,
    /// Episode indexes refer to this ordered source inventory.
    pub events: Vec<ContentAction>,
    pub limitations: Vec<String>,
    pub episodes: Vec<InvestigationEpisode>,
    pub complete: bool,
}

/// Use a branch-scoped projection and the snapshot from the same
/// publication. Spans are disjoint, ordered, and anchored in stable records.
pub fn build_episodes(
    content: &SessionContentEvidence,
    task: &SessionScopeSnapshot,
    spans: &[EpisodeSpan],
) -> Result<OverExploringInput, JevError> {
    if content.publication_fence != task.publication_fence()
        || content.session_identity_digest.is_empty()
        || content.actions.len() > MAX_EVENTS
        || spans.len() > MAX_EPISODES
    {
        return Err(JevError::InvalidCheckContext);
    }
    let actions = &content.actions;
    let mut limitations = content.limitations.clone();
    let mut action_bytes = 0usize;
    for action in actions {
        if action.text.len() > MAX_INPUT_BYTES {
            return Err(JevError::InvalidCheckContext);
        }
        let bytes = serde_json::to_vec(action)
            .map_err(|_| JevError::RequestSerialization)?
            .len();
        action_bytes = action_bytes
            .checked_add(bytes)
            .ok_or(JevError::InvalidCheckContext)?;
        if action_bytes > MAX_INPUT_BYTES {
            return Err(JevError::InvalidCheckContext);
        }
    }
    let mut ids = BTreeSet::new();
    let branch = actions.first().map(|action| {
        (
            &action.reference.source_key_digest,
            &action.reference.thread_digest,
        )
    });
    for action in actions {
        if !ids.insert(&action.reference.id)
            || Some((
                &action.reference.source_key_digest,
                &action.reference.thread_digest,
            )) != branch
            || action.turn_role == "thinking"
            || action.kind == "thinking"
        {
            return Err(JevError::InvalidCheckContext);
        }
    }
    if actions
        .windows(2)
        .any(|pair| position(&pair[0]) >= position(&pair[1]))
        || task.occurrences().iter().any(|item| {
            Some((
                &item.reference.source_key_digest,
                &item.reference.thread_digest,
            )) != branch
        })
    {
        return Err(JevError::InvalidCheckContext);
    }
    if actions.iter().any(|action| {
        action.kind == "user"
            && action.authority == "user"
            && !task.occurrences().iter().any(|item| {
                item.reference.id == action.reference.id && item.authority == ScopeAuthority::User
            })
    }) {
        limitations.push("task_record_not_retained".into());
    }
    let mut episodes = Vec::new();
    let mut previous_end = None;
    let mut covered_requests = BTreeSet::new();
    for span in spans {
        let start = actions
            .iter()
            .position(|action| action.reference.id == span.first_event_id)
            .ok_or(JevError::InvalidCheckContext)?;
        let end = actions
            .iter()
            .position(|action| action.reference.id == span.last_event_id)
            .ok_or(JevError::InvalidCheckContext)?;
        if start > end || previous_end.is_some_and(|previous| previous >= start) {
            return Err(JevError::InvalidCheckContext);
        }
        previous_end = Some(end);
        let events = &actions[start..=end];
        let mut reads = Vec::new();
        for event in events {
            if event.context_only {
                continue;
            }
            let Some(request) = &event.metadata.read_request else {
                continue;
            };
            if request.reference_id != event.reference.id
                || event.kind != "tool_input"
                || !event.reference.stable
                || request.paths.len() != 1
            {
                limitations.push("read_request_unavailable".into());
                covered_requests.insert(request.reference_id.clone());
                continue;
            }
            covered_requests.insert(request.reference_id.clone());
            let matches: Vec<_> = events
                .iter()
                .filter(|result| {
                    result.metadata.read_result.as_ref().is_some_and(|read| {
                        read.request_reference_id.as_deref() == Some(&request.reference_id)
                    })
                })
                .collect();
            let mut matched = if matches.len() == 1 {
                matches.first().copied()
            } else {
                None
            };
            if matches.len() > 1 {
                limitations.push("read_result_ambiguous".into());
            }
            if let Some(result) = matched {
                let read = result
                    .metadata
                    .read_result
                    .as_ref()
                    .ok_or(JevError::InvalidCheckContext)?;
                if !result.reference.stable
                    || result.kind != "tool_result"
                    || read.reference_id != result.reference.id
                    || result.tool_call_id.is_none()
                    || result.tool_call_id != event.tool_call_id
                    || result.tool_name != event.tool_name
                    || position(result) <= position(event)
                    || read.recorded_output_digest != sha256_hex(result.text.as_bytes())
                    || read.recorded_output_bytes
                        != u64::try_from(result.text.len())
                            .map_err(|_| JevError::InvalidCheckContext)?
                {
                    limitations.push("read_result_binding_invalid".into());
                    matched = None;
                }
            }
            reads.push(ReadObservation {
                request: request.clone(),
                result: matched.and_then(|event| event.metadata.read_result.clone()),
            });
        }
        if reads.is_empty() {
            continue;
        }
        let first_read = &reads[0].request.reference_id;
        let id = StableId::new(
            "over-exploring-episode-v1",
            &[
                content.session_identity_digest.as_bytes(),
                first_read.as_bytes(),
            ],
        );
        episodes.push(InvestigationEpisode {
            id,
            state: span.state,
            before: (0..start).collect(),
            events: (start..=end).collect(),
            subsequent: (end + 1..actions.len()).collect(),
            reads,
        });
    }
    if actions.iter().any(|action| {
        !action.context_only
            && action
                .metadata
                .read_request
                .as_ref()
                .is_some_and(|request| !covered_requests.contains(&request.reference_id))
    }) {
        return Err(JevError::InvalidCheckContext);
    }
    let task_contexts = episodes
        .iter()
        .map(|episode| {
            let anchor = &actions[episode.events[0]];
            (
                episode.id,
                task_context(
                    task,
                    anchor,
                    &actions[*episode.events.last().expect("nonempty episode")],
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let task_context = task_contexts
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| task.user_context());
    let input = OverExploringInput {
        session_identity: content.session_identity_digest.clone(),
        task_context,
        task_contexts,
        events: actions.clone(),
        limitations,
        episodes,
        complete: content.complete,
    };
    if serde_json::to_vec(&input)
        .map_err(|_| JevError::RequestSerialization)?
        .len()
        > MAX_INPUT_BYTES
    {
        return Err(JevError::InvalidCheckContext);
    }
    Ok(input)
}

fn task_context(
    task: &SessionScopeSnapshot,
    anchor: &ContentAction,
    end: &ContentAction,
) -> JevSharedRequestContext {
    let start = task
        .occurrences()
        .iter()
        .filter(|item| {
            item.authority == ScopeAuthority::User
                && item.field == JevInputField::UserMessage
                && (item.reference.turn_index, item.reference.part_index) <= position(anchor)
        })
        .map(|item| (item.reference.turn_index, item.reference.part_index))
        .max();
    let mut values = Vec::new();
    let mut evidence = Vec::new();
    let antecedent_context_omitted = task.occurrences().iter().any(|item| {
        item.authority == ScopeAuthority::User
            && item.field == JevInputField::UserMessage
            && start
                .is_some_and(|start| (item.reference.turn_index, item.reference.part_index) < start)
    });
    let mut partial = antecedent_context_omitted;
    for (source_index, item) in task.occurrences().iter().enumerate().filter(|(_, item)| {
        let item_position = (item.reference.turn_index, item.reference.part_index);
        start.is_some_and(|start| item_position >= start)
            && item_position <= position(end)
            && item.field != JevInputField::AssistantMessage
    }) {
        let value = &task.values()[item.value_index];
        let text = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        let chunks = crate::analysis::jev::text_ranges::text_ranges(&text, 512, 0);
        let ranges = if chunks.len() <= 4 {
            chunks
        } else {
            [0, chunks.len() / 3, chunks.len() / 2, chunks.len() - 1]
                .into_iter()
                .map(|index| chunks[index])
                .collect()
        };
        partial |= ranges.iter().map(|(start, end)| end - start).sum::<usize>() < text.len();
        values.push(if ranges.iter().map(|(start,end)| end-start).sum::<usize>() == text.len() { value.clone() } else { json!({"field": item.field, "authority": item.authority, "chunks": ranges.iter().map(|&(start,end)| json!({"start":start,"end":end,"text": &text[start..end]})).collect::<Vec<_>>(), "total_bytes":text.len()}) });
        evidence.push(JevEvidenceReference {
            part_id: format!("shared_context.occurrences[{source_index}]"),
            source_id: item.reference.id.clone(),
            content_kind: format!("{:?}", item.field),
            role: if item.authority == ScopeAuthority::User {
                JevEvidenceRole::Instruction
            } else {
                JevEvidenceRole::SupportingContext
            },
        });
    }
    evidence.sort_by(|left, right| left.source_id.cmp(&right.source_id));
    JevSharedRequestContext {
        fields: json!({"values": values, "partial": partial, "antecedent_context_omitted": antecedent_context_omitted, "limitations": task.limitations()}),
        evidence,
    }
}

fn position(action: &ContentAction) -> (u64, u32) {
    (action.reference.turn_index, action.reference.part_index)
}

pub fn build_jev_context(input: &OverExploringInput) -> Result<JevSessionContext, JevError> {
    let bytes = serde_json::to_vec(input).map_err(|_| JevError::RequestSerialization)?;
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(JevError::InvalidCheckContext);
    }
    Ok(JevSessionContext {
        input_revision: sha256_hex(&bytes),
        session_identity: input.session_identity.clone(),
        check_context: json!(input),
        limitations: input.limitations.clone(),
        reference_snapshots: Vec::new(),
        evidence_store: Default::default(),
    })
}
