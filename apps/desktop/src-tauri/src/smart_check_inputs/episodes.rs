use antiburn_local::analysis::jev::JevInputField;
use antiburn_local::analysis::jev_evidence::{
    ContentAction, JevOperationState, SessionContentEvidence, is_recorded_skill_selection,
};
use antiburn_local::analysis::session_scope::{ScopeAuthority, SessionScopeSnapshot};
use antiburn_local::checks::over_exploring::{EpisodeSpan, EpisodeState};

use super::{InputLoadError, InputUnavailable, unavailable};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpisodeCompletion {
    NextAuthoritativeUser {
        reference_id: String,
    },
    /// Completed task activity follows the reads. This does not prove useful work.
    RecordedOperation {
        reference_id: String,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvestigationSpan {
    pub span: EpisodeSpan,
    pub completion: EpisodeCompletion,
}

pub(super) fn investigation_spans(
    content: &SessionContentEvidence,
    scope: &SessionScopeSnapshot,
) -> Result<Vec<InvestigationSpan>, InputLoadError> {
    let actions = &content.actions;
    let mut starts = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        if action.kind != "user" {
            continue;
        }
        if is_recorded_skill_selection(action)
            || action
                .metadata
                .non_authorizing_context
                .as_ref()
                .is_some_and(|fact| fact.matches_action(action, content.source_format))
        {
            continue;
        }
        if action.authority != "user"
            || !scope.occurrences().iter().any(|item| {
                item.authority == ScopeAuthority::User && item.reference.id == action.reference.id
            })
        {
            return Err(unavailable(InputUnavailable::IncompleteEvidence));
        }
        if starts.last().is_none_or(|previous: &usize| {
            actions[*previous].reference.turn_index != action.reference.turn_index
        }) {
            starts.push(index);
        }
    }
    let mut spans = Vec::new();
    for (offset, start) in starts.iter().copied().enumerate() {
        let next = starts.get(offset + 1).copied();
        let end = next.unwrap_or(actions.len()) - 1;
        let events = &actions[start..=end];
        if !events
            .iter()
            .any(|event| event.metadata.read_request.is_some())
        {
            continue;
        }
        let completion = match next {
            Some(next) if recorded_operations_complete(events) => {
                EpisodeCompletion::NextAuthoritativeUser {
                    reference_id: actions[next].reference.id.clone(),
                }
            }
            None if recorded_operations_complete(events) && task_continues_after_reads(events) => {
                EpisodeCompletion::RecordedOperation {
                    reference_id: actions[end].reference.id.clone(),
                }
            }
            _ => EpisodeCompletion::Unknown,
        };
        if spans.len() == 256 {
            return Err(unavailable(InputUnavailable::AssemblyLimitReached));
        }
        spans.push(InvestigationSpan {
            span: EpisodeSpan {
                first_event_id: actions[start].reference.id.clone(),
                last_event_id: actions[end].reference.id.clone(),
                state: if completion == EpisodeCompletion::Unknown {
                    EpisodeState::Deferred
                } else {
                    EpisodeState::Complete
                },
            },
            completion,
        });
    }
    Ok(spans)
}

fn recorded_operations_complete(events: &[ContentAction]) -> bool {
    if events
        .iter()
        .filter(|event| event.kind == "tool_result")
        .any(|result| {
            result.tool_call_id.is_none()
                || events
                    .iter()
                    .filter(|request| {
                        request.kind == "tool_input"
                            && request.tool_call_id == result.tool_call_id
                            && request.tool_name == result.tool_name
                            && (request.reference.turn_index, request.reference.part_index)
                                < (result.reference.turn_index, result.reference.part_index)
                    })
                    .count()
                    != 1
        })
    {
        return false;
    }
    events
        .iter()
        .filter(|event| event.kind == "tool_input")
        .all(|request| {
            if request.metadata.state == JevOperationState::Running {
                return false;
            }
            let mut results = events.iter().filter(|result| {
                result.kind == "tool_result"
                    && result.tool_call_id.is_some()
                    && result.tool_call_id == request.tool_call_id
                    && result.tool_name == request.tool_name
                    && (result.reference.turn_index, result.reference.part_index)
                        > (request.reference.turn_index, request.reference.part_index)
            });
            results.next().is_some_and(|result| {
                result.metadata.state == JevOperationState::Completed
                    || (request.metadata.state == JevOperationState::Completed
                        && result.metadata.state == JevOperationState::Unknown)
            }) && results.next().is_none()
        })
}

fn task_continues_after_reads(events: &[ContentAction]) -> bool {
    let Some(last_read) = events
        .iter()
        .rposition(|event| event.metadata.read_result.is_some())
    else {
        return false;
    };
    events[last_read + 1..].iter().any(|event| {
        event.kind == "tool_input"
            && event
                .normalized_fields
                .as_ref()
                .is_some_and(|fields| fields.values.contains_key(&JevInputField::FileEditPath))
    }) && events
        .last()
        .is_some_and(|event| event.kind == "tool_result")
}
