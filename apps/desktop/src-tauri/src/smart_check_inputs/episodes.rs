use antiburn_local::analysis::jev_evidence::{
    ContentAction, JevOperationState, SessionContentEvidence, is_recorded_skill_selection,
};
use antiburn_local::analysis::session_scope::{ScopeAuthority, SessionScopeSnapshot};
use antiburn_local::checks::over_exploring::{EpisodeSpan, EpisodeState};

pub(super) const MAX_INVESTIGATION_SPANS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpisodeCompletion {
    NextAuthoritativeUser {
        reference_id: String,
    },
    /// Completed task activity follows the reads. This does not prove useful work.
    RecordedOperation {
        reference_id: String,
    },
    ClosedDuplicateInterval {
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
) -> (Vec<InvestigationSpan>, bool) {
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
            continue;
        }
        if starts.last().is_none_or(|previous: &usize| {
            actions[*previous].reference.turn_index != action.reference.turn_index
        }) {
            starts.push(index);
        }
    }
    let first_user = starts.first().copied().unwrap_or(actions.len());
    if actions[..first_user]
        .iter()
        .any(|action| action.metadata.read_request.is_some())
    {
        starts.insert(0, 0);
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
            None if content.complete
                && content.limitations.is_empty()
                && recorded_operations_complete(events)
                && closed_duplicate_interval(events) =>
            {
                EpisodeCompletion::ClosedDuplicateInterval {
                    reference_id: actions[end].reference.id.clone(),
                }
            }
            _ => EpisodeCompletion::Unknown,
        };
        spans.push(InvestigationSpan {
            span: EpisodeSpan {
                first_event_id: actions[start].reference.id.clone(),
                last_event_id: actions[end].reference.id.clone(),
                state: if matches!(
                    completion,
                    EpisodeCompletion::Unknown | EpisodeCompletion::ClosedDuplicateInterval { .. }
                ) {
                    EpisodeState::Deferred
                } else {
                    EpisodeState::Complete
                },
            },
            completion,
        });
    }
    let sampled = spans.len() > MAX_INVESTIGATION_SPANS;
    if sampled {
        spans.drain(..spans.len() - MAX_INVESTIGATION_SPANS);
    }
    (spans, sampled)
}

fn closed_duplicate_interval(events: &[ContentAction]) -> bool {
    use antiburn_local::analysis::jev_evidence::{JevReadResultKind, JevReadStatus};
    let requests = events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.metadata.read_request.is_some())
        .collect::<Vec<_>>();
    requests.windows(2).any(|pair| {
        let (earlier_index, earlier) = pair[0];
        let (later_index, later) = pair[1];
        let first = earlier
            .metadata
            .read_request
            .as_ref()
            .expect("read request");
        let second = later.metadata.read_request.as_ref().expect("read request");
        if !earlier.reference.stable
            || !later.reference.stable
            || first.truncated
            || second.truncated
            || first.paths.len() != 1
            || first.paths != second.paths
            || first.cwd != second.cwd
            || (first.cwd.is_none()
                && !antiburn_local::checks::over_exploring::is_absolute_source_path(
                    &first.paths[0],
                ))
            || first.extent != second.extent
            || first.native_extent != second.native_extent
        {
            return false;
        }
        let bound = |request: &ContentAction| {
            events.iter().enumerate().find(|(_, event)| {
                event.metadata.read_result.as_ref().is_some_and(|result| {
                    result.request_reference_id.as_deref() == Some(request.reference.id.as_str())
                }) && event.tool_call_id.is_some()
                    && event.tool_call_id == request.tool_call_id
                    && event.tool_name == request.tool_name
                    && event.reference.stable
            })
        };
        let Some((first_index, first_action)) = bound(earlier) else {
            return false;
        };
        let Some((second_index, second_action)) = bound(later) else {
            return false;
        };
        let first_result = first_action
            .metadata
            .read_result
            .as_ref()
            .expect("bound result");
        let second_result = second_action
            .metadata
            .read_result
            .as_ref()
            .expect("bound result");
        if first.reference_id != earlier.reference.id
            || second.reference_id != later.reference.id
            || [(first_action, first_result), (second_action, second_result)]
                .iter()
                .any(|(action, result)| {
                    result.reference_id != action.reference.id
                        || Some(result.recorded_output_bytes)
                            != u64::try_from(action.text.len()).ok()
                        || result.recorded_output_digest
                            != antiburn_local::checks::ignored_instructions::sha256_hex(
                                action.text.as_bytes(),
                            )
                })
        {
            return false;
        }
        earlier_index < first_index
            && first_index < later_index
            && later_index < second_index
            && events[first_index + 1..later_index].iter().all(|event| {
                event.kind == "assistant" && event.context_only && event.text.trim().is_empty()
            })
            && [first_result, second_result].iter().all(|result| {
                result.status == JevReadStatus::Success
                    && result.kind == JevReadResultKind::File
                    && !result.truncated
                    && antiburn_local::checks::over_exploring::has_known_returned_extent(result)
            })
            && !first_action.truncated
            && !second_action.truncated
            && first_result.returned_extent == second_result.returned_extent
            && first_result.recorded_output_digest == second_result.recorded_output_digest
    })
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
            && event.metadata.read_request.is_none()
            && event.metadata.state == JevOperationState::Completed
    }) && events
        .last()
        .is_some_and(|event| event.kind == "tool_result")
}
