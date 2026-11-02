use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::bounded_json::measure_json;
use crate::context::ObservedAttribution;
use crate::domain::{
    BranchId, EvidenceDependency, EvidenceVersion, FrozenContext, Message, NativeOrderKey,
    ObservationDraft, ObservationKind,
};

use super::messages::{PUBLIC_LIMITS, add_attribution, invalid};
use super::{
    EvidenceValidity, PublicationCursor, PublishedContext, ResponseCycleStatus, SessionStartReason,
    SourceError, SourceEvent, validate_public_message,
};

/// An immutable admitted public or host event. Corrections retain its identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedObservation {
    pub identity: String,
    pub native_order: NativeOrderKey,
    pub first_eligible_publication: Option<PublicationCursor>,
    pub revision: u64,
    pub eligible: bool,
    pub payload_complete: bool,
    pub validity: EvidenceValidity,
    pub payload: PublishedObservationPayload,
}

/// Closed publication alternatives. Native objects never become public roots.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum PublishedObservationPayload {
    UserMessage(Message),
    AssistantMessage(Message),
    ToolCall(Message),
    ResponseCycle(PublishedResponseCycle),
    SessionStart {
        reason: SessionStartReason,
    },
    SessionIdle {
        attachment_epoch: String,
        activity_revision: u64,
        idle_seconds: u32,
        last_activity_at_ms: Option<i64>,
    },
}

/// A complete source-proven cycle and its exact current member bindings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedResponseCycle {
    pub id: String,
    pub status: ResponseCycleStatus,
    pub messages: Vec<Message>,
    pub attribution: ObservedAttribution,
    pub native_reason: Option<String>,
    pub native_error_code: Option<String>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub dependencies: Vec<PublishedSourceEvidence>,
}

/// One exact native current version, independent of selected observations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedSourceEvidence {
    pub identity: String,
    pub version: EvidenceVersion,
}

pub(super) fn normalize_observation(
    cursor: PublicationCursor,
    branch: BranchId,
    context: Option<PublishedContext>,
    source_evidence_version: Option<EvidenceVersion>,
    published: PublishedObservation,
) -> Result<SourceEvent, SourceError> {
    let unsafe_update = |reason: String| SourceEvent::UnsafeUpdate {
        cursor,
        branch: branch.clone(),
        reason,
    };
    let Some(context) = context.filter(|context| {
        context.main_session
            && !context.session_id.trim().is_empty()
            && !context.agent_name.trim().is_empty()
    }) else {
        return Ok(unsafe_update(
            "The public event lacks admitted main-session context.".into(),
        ));
    };
    if published.validity != EvidenceValidity::Valid
        || branch.0.trim().is_empty()
        || published.identity.trim().is_empty()
        || published.native_order.0.trim().is_empty()
        || published
            .first_eligible_publication
            .is_some_and(|first| first > cursor)
    {
        return Ok(unsafe_update(
            "The public event lacks safe identity, ownership, or order.".into(),
        ));
    }
    if measure_json(&(&published, &context), PUBLIC_LIMITS).is_err() {
        return Ok(unsafe_update(
            "The public event or context exceeds its bounds.".into(),
        ));
    }
    let encoded = serde_json::to_vec(&(&branch, &published, &context))
        .map_err(|error| SourceError::Reader(error.to_string()))?;
    let version = EvidenceVersion(
        Sha256::digest(encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    );
    let mut frozen = match public_context(&context) {
        Ok(context) => context,
        Err(error) => return Ok(unsafe_update(format!("Invalid public context: {error:?}"))),
    };
    let kind = match if published.eligible {
        normalize_payload(&published, &context, &mut frozen)
    } else {
        Ok(ObservationKind::Other)
    } {
        Ok(kind) => kind,
        Err(error) => return Ok(unsafe_update(format!("Invalid public event: {error:?}"))),
    };
    if !matches!(
        kind,
        ObservationKind::UserMessage(_)
            | ObservationKind::AssistantMessage(_)
            | ObservationKind::ToolCall(_)
    ) {
        frozen
            .dependencies
            .push(EvidenceDependency::SourceEvidence {
                id: published.identity.clone(),
                version: version.clone(),
            });
    }
    Ok(SourceEvent::Observation {
        cursor,
        source_evidence_version,
        identity: published.identity,
        revision: published.revision,
        observation: Box::new(ObservationDraft {
            branch,
            native_order: published.native_order,
            first_terminal_publication: None,
            first_eligible_publication: published.first_eligible_publication,
            version,
            kind,
            context: frozen,
            complete: published.eligible && published.payload_complete,
        }),
    })
}

fn normalize_payload(
    published: &PublishedObservation,
    context: &PublishedContext,
    frozen: &mut FrozenContext,
) -> Result<ObservationKind, SourceError> {
    Ok(match &published.payload {
        PublishedObservationPayload::UserMessage(message)
        | PublishedObservationPayload::AssistantMessage(message)
        | PublishedObservationPayload::ToolCall(message) => {
            let kind = match published.payload {
                PublishedObservationPayload::UserMessage(_) => "user_message",
                PublishedObservationPayload::AssistantMessage(_) => "assistant_message",
                _ => "tool_call",
            };
            validate_public_message(message, kind)?;
            if message.id != published.identity
                || message.value["source"].as_str() != Some(&context.agent_name)
            {
                return Err(invalid(
                    "The message does not match its publication identity or source.",
                ));
            }
            match published.payload {
                PublishedObservationPayload::UserMessage(_) => {
                    ObservationKind::UserMessage(message.clone())
                }
                PublishedObservationPayload::AssistantMessage(_) => {
                    ObservationKind::AssistantMessage(message.clone())
                }
                _ => ObservationKind::ToolCall(message.clone()),
            }
        }
        PublishedObservationPayload::ResponseCycle(cycle) => {
            if !published.payload_complete
                || cycle.id != published.identity
                || cycle.messages.is_empty()
                || cycle.messages.len() > 4096
                || cycle.dependencies.len() > 4096
            {
                return Err(invalid(
                    "The response cycle is incomplete or exceeds its member bound.",
                ));
            }
            let mut members = std::collections::BTreeSet::new();
            for message in &cycle.messages {
                let kind = message
                    .value
                    .get("kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("A cycle member has no kind."))?;
                validate_public_message(message, kind)?;
                if message.value["source"].as_str() != Some(&context.agent_name) {
                    return Err(invalid("A cycle member has a different source."));
                }
                if !members.insert(message.id.as_str()) {
                    return Err(invalid("A cycle cannot repeat a current message identity."));
                }
            }
            let mut bindings = std::collections::BTreeMap::new();
            for binding in &cycle.dependencies {
                if binding.version.0.trim().is_empty()
                    || bindings
                        .insert(binding.identity.as_str(), &binding.version)
                        .is_some()
                {
                    return Err(invalid("A cycle dependency is missing or duplicated."));
                }
            }
            if members.len() != bindings.len()
                || members.iter().any(|id| !bindings.contains_key(id))
            {
                return Err(invalid(
                    "The cycle requires exact current bindings for every public member.",
                ));
            }
            let mut turn = json!({"id": cycle.id, "source": context.agent_name, "status": cycle.status, "messages": cycle.messages.iter().map(|message| &message.value).collect::<Vec<_>>()});
            add_attribution(&mut turn, &cycle.attribution);
            if cycle
                .attribution
                .model
                .as_ref()
                .is_some_and(|model| !crate::filters::is_model_identifier(model))
            {
                return Err(invalid(
                    "The cycle attribution has an invalid model identifier.",
                ));
            }
            for (field, time) in [
                ("started_at", cycle.started_at_ms),
                ("ended_at", cycle.ended_at_ms),
            ] {
                if let Some(time) = time {
                    turn[field] = json!(time);
                }
            }
            frozen
                .dependencies
                .extend(cycle.dependencies.iter().map(|binding| {
                    EvidenceDependency::SourceEvidence {
                        id: binding.identity.clone(),
                        version: binding.version.clone(),
                    }
                }));
            frozen.value["turn"] = turn.clone();
            ObservationKind::TurnEnd { turn }
        }
        PublishedObservationPayload::SessionStart { reason } => {
            frozen.value["session"]["start_reason"] = json!(reason);
            ObservationKind::SessionStart { reason: *reason }
        }
        PublishedObservationPayload::SessionIdle {
            attachment_epoch,
            activity_revision,
            idle_seconds,
            last_activity_at_ms,
        } => {
            if attachment_epoch.trim().is_empty() || *idle_seconds < 120 {
                return Err(invalid(
                    "An idle boundary requires an attachment and at least 120 seconds.",
                ));
            }
            frozen.value["session"]["idle_seconds"] = json!(idle_seconds);
            if let Some(time) = last_activity_at_ms {
                frozen.value["session"]["last_activity_at"] = json!(time);
            }
            frozen
                .dependencies
                .push(EvidenceDependency::SessionActivity {
                    attachment_epoch: attachment_epoch.clone(),
                    activity_revision: *activity_revision,
                });
            ObservationKind::SessionIdle {
                attachment_epoch: attachment_epoch.clone(),
                activity_revision: *activity_revision,
                idle_seconds: *idle_seconds,
                last_activity_at_ms: *last_activity_at_ms,
            }
        }
    })
}

fn public_context(context: &PublishedContext) -> Result<FrozenContext, SourceError> {
    let requests = context
        .requests
        .iter()
        .map(|(id, value)| {
            let source = value
                .get("source")
                .and_then(Value::as_str)
                .filter(|source| !source.trim().is_empty())
                .ok_or_else(|| invalid("A request requires its source."))?;
            let kind = value
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("user_message");
            let text = value
                .get("text")
                .or_else(|| value.get("content"))
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("A request requires public text."))?;
            if id.trim().is_empty()
                || !matches!(kind, "user_message" | "question_answer" | "approval")
            {
                return Err(invalid("The request identity or kind is invalid."));
            }
            let mut request = json!({"id": id, "source": source, "kind": kind, "text": text});
            if let Some(reply) = value.get("in_reply_to") {
                if !reply.is_string() {
                    return Err(invalid("The request reply binding must be a string."));
                }
                request["in_reply_to"] = reply.clone();
            }
            Ok(request)
        })
        .collect::<Result<Vec<_>, SourceError>>()?;
    let mut session = json!({"id": context.session_id, "requests": requests, "requests_complete": context.complete_through_event, "source_coverage": if context.complete_through_event { "complete" } else { "partial" }});
    if let Some(selection) = &context.session_selection {
        if selection
            .model
            .as_ref()
            .is_some_and(|model| !crate::filters::is_model_identifier(model))
        {
            return Err(invalid("The current selection model is invalid."));
        }
        add_attribution(&mut session, selection);
    }
    let mut value = json!({"session": session, "agent": {"name": context.agent_name}, "global": {}, "repo": {}});
    context.inventories.merge_into(&mut value);
    if let Some(root) = &context.project_root {
        value["repo"]["root"] = json!(root);
    } else {
        value["repo"] = json!({"instructions_complete": false, "instructions_limits": ["repository-root-unavailable"], "skills_complete": false, "skills_limits": ["repository-root-unavailable"]});
    }
    Ok(FrozenContext {
        value,
        dependencies: vec![EvidenceDependency::Context {
            id: "user-messages-through-event".into(),
            version: EvidenceVersion(context.version.clone()),
        }],
    })
}
