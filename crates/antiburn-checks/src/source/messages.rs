use serde_json::{Value, json};

use crate::bounded_json::JsonLimits;
use crate::context::ObservedAttribution;
use crate::domain::{Message, ObservationKind};

use super::{
    PublicationCursor, PublishedContext, PublishedEvidenceChange, PublishedEvidenceEvent,
    PublishedToolMetadata, PublishedToolResult, SourceError, SourceEvent, validate_public_message,
};

pub(super) const PUBLIC_LIMITS: JsonLimits = JsonLimits {
    max_bytes: 4 * 1024 * 1024,
    max_nodes: 262_144,
    max_depth: 64,
};

/// Builds one public user message. User messages have no generator attribution.
pub fn normalize_user_message(
    id: &str,
    source: &str,
    content: &str,
    complete: bool,
) -> Result<Message, SourceError> {
    text_message(id, source, content, complete, "user_message", "user", None)
}

/// Builds one complete public assistant text value, without native reasoning fields.
pub fn normalize_assistant_message(
    id: &str,
    source: &str,
    content: &str,
    complete: bool,
    attribution: &ObservedAttribution,
) -> Result<Message, SourceError> {
    text_message(
        id,
        source,
        content,
        complete,
        "assistant_message",
        "assistant",
        Some(attribution),
    )
}

fn text_message(
    id: &str,
    source: &str,
    content: &str,
    complete: bool,
    kind: &str,
    role: &str,
    attribution: Option<&ObservedAttribution>,
) -> Result<Message, SourceError> {
    let mut value = json!({"id": id, "source": source, "kind": kind, "role": role, "content": content, "quality": {"source_complete": complete}});
    if let Some(attribution) = attribution {
        add_attribution(&mut value, attribution);
    }
    let message = Message {
        id: id.into(),
        value,
    };
    validate_public_message(&message, kind)?;
    Ok(message)
}

/// Builds an explicit call. Text input remains a JSON string in `arguments`.
#[allow(clippy::too_many_arguments)]
pub fn normalize_tool_call(
    id: &str,
    source: &str,
    call_id: &str,
    tool_name: &str,
    arguments: Option<Value>,
    metadata: &PublishedToolMetadata,
    provider_executed: Option<bool>,
    complete: bool,
) -> Result<Message, SourceError> {
    let mut value = json!({"id": id, "source": source, "kind": "tool_call", "role": "assistant", "tool": {"call_id": call_id, "name": tool_name}, "quality": {
        "source_complete": complete, "arguments_available": arguments.is_some(), "paths_available": metadata.paths.is_some(), "paths_complete": metadata.paths.is_some() && metadata.paths_complete, "paths_limits": metadata.paths_limits,
    }});
    if let Some(arguments) = arguments {
        value["tool"]["arguments"] = arguments;
    }
    if let Some(category) = metadata.check_category {
        value["tool"]["category"] = json!(category.as_str());
    }
    if let Some(paths) = &metadata.paths {
        value["tool"]["paths"] = json!(paths);
    }
    if let Some(executed) = provider_executed {
        value["tool"]["provider_executed"] = json!(executed);
    }
    add_attribution(&mut value, &metadata.attribution);
    if metadata.attribution.fast_mode.is_some()
        && let Some(evidence) = &metadata.fast_mode_evidence
    {
        value["quality"]["fast_mode_evidence"] = json!(evidence);
    }
    for (field, fact) in [
        ("model_limit", &metadata.model_limit),
        ("fast_mode_limit", &metadata.fast_mode_limit),
        ("category_limit", &metadata.category_limit),
    ] {
        if let Some(fact) = fact {
            value["quality"][field] = json!(fact);
        }
    }
    let message = Message {
        id: id.into(),
        value,
    };
    validate_public_message(&message, "tool_call")?;
    Ok(message)
}

/// Uses the result publication path for both standalone and inline result values.
pub fn normalize_tool_result_message(
    result: &PublishedToolResult,
    context: &PublishedContext,
) -> Result<Message, SourceError> {
    let event = PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(0),
        branch: "public-message-normalization".into(),
        context: Some(context.clone()),
        change: PublishedEvidenceChange::Upsert(result.clone()),
    };
    match super::normalize(event)? {
        SourceEvent::Observation { observation, .. } => match observation.kind {
            ObservationKind::ToolResult { message, .. } => Ok(message),
            _ => Err(invalid("A cycle result must be terminal.")),
        },
        _ => Err(invalid("The result lacks safe source admission.")),
    }
}

pub(super) fn add_attribution(value: &mut Value, attribution: &ObservedAttribution) {
    if let Some(model) = &attribution.model {
        value["model"] = json!(model);
    }
    if let Some(mode) = attribution.fast_mode {
        value["fast_mode"] = json!(mode);
    }
}

pub(super) fn invalid(message: &str) -> SourceError {
    SourceError::InvalidObservation(message.into())
}
