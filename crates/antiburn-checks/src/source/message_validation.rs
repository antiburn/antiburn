use serde_json::Value;

use crate::bounded_json::measure_json;
use crate::domain::Message;

use super::SourceError;
use super::messages::{PUBLIC_LIMITS, invalid};

/// Validates a typed, closed public message before it enters an event or cycle.
pub fn validate_public_message(message: &Message, expected_kind: &str) -> Result<(), SourceError> {
    measure_json(&message.value, PUBLIC_LIMITS)
        .map_err(|_| invalid("The public message exceeds its bounds."))?;
    validate_envelope(message, expected_kind)?;
    let value = &message.value;
    if let Some(quality) = value.get("quality") {
        validate_quality(quality)?;
    }
    if matches!(expected_kind, "user_message" | "assistant_message") {
        if !value.get("content").is_some_and(Value::is_string) || value.get("tool").is_some() {
            return Err(invalid(
                "A text message requires public content and cannot contain a tool.",
            ));
        }
        if expected_kind == "user_message"
            && (value.get("model").is_some() || value.get("fast_mode").is_some())
        {
            return Err(invalid("User messages cannot have generator attribution."));
        }
        Ok(())
    } else {
        if value.get("content").is_some() {
            return Err(invalid("Tool messages cannot contain public text fields."));
        }
        let tool = value
            .get("tool")
            .ok_or_else(|| invalid("A tool message requires tool facts."))?;
        validate_tool(tool, expected_kind)
    }
}

fn validate_envelope(message: &Message, expected_kind: &str) -> Result<(), SourceError> {
    let value = &message.value;
    closed(
        value,
        &[
            "id",
            "source",
            "occurred_at",
            "kind",
            "role",
            "model",
            "fast_mode",
            "content",
            "tool",
            "quality",
            "native_order",
        ],
    )?;
    if required_text(value, "id")? != message.id || required_text(value, "kind")? != expected_kind {
        return Err(invalid(
            "The typed public message kind or identity does not match its value.",
        ));
    }
    required_text(value, "source")?;
    let role = match expected_kind {
        "user_message" => "user",
        "assistant_message" | "tool_call" => "assistant",
        "tool_result" => "tool",
        _ => return Err(invalid("The public message kind is unavailable.")),
    };
    if value.get("role").and_then(Value::as_str) != Some(role) {
        return Err(invalid("The public message role does not match its kind."));
    }
    if let Some(model) = value.get("model")
        && !model
            .as_str()
            .is_some_and(crate::filters::is_model_identifier)
    {
        return Err(invalid("The recorded model identifier is invalid."));
    }
    if value
        .get("fast_mode")
        .is_some_and(|mode| !mode.is_boolean())
    {
        return Err(invalid("Recorded mode must be a Boolean."));
    }
    if value
        .get("occurred_at")
        .is_some_and(|time| !time.is_string() && time.as_i64().is_none())
    {
        return Err(invalid(
            "The source time must be a string or integer timestamp.",
        ));
    }
    if let Some(order) = value.get("native_order")
        && (expected_kind != "tool_result" || !order.is_string())
    {
        return Err(invalid(
            "Only legacy results can contain a native-order string.",
        ));
    }
    Ok(())
}

fn validate_quality(quality: &Value) -> Result<(), SourceError> {
    closed(
        quality,
        &[
            "source_complete",
            "tool_name_available",
            "category_available",
            "arguments_available",
            "result_content_available",
            "request_history_complete",
            "main_session",
            "paths_available",
            "paths_complete",
            "paths_limits",
            "model_limit",
            "fast_mode_limit",
            "fast_mode_evidence",
            "category_limit",
        ],
    )?;
    for (key, fact) in quality.as_object().expect("quality was checked") {
        let valid = match key.as_str() {
            "paths_limits" => fact
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
            "model_limit" | "fast_mode_limit" | "fast_mode_evidence" | "category_limit" => {
                fact.is_string()
            }
            _ => fact.is_boolean(),
        };
        if !valid {
            return Err(invalid("A public quality fact has an invalid type."));
        }
    }
    Ok(())
}

fn validate_tool(tool: &Value, kind: &str) -> Result<(), SourceError> {
    closed(
        tool,
        &[
            "call_id",
            "name",
            "category",
            "arguments",
            "paths",
            "provider_executed",
            "result",
        ],
    )?;
    for field in ["call_id", "name"] {
        if kind == "tool_call" || tool.get(field).is_some() {
            required_text(tool, field)?;
        }
    }
    if tool.get("category").is_some_and(|category| {
        !category.as_str().is_some_and(|category| {
            matches!(
                category,
                "shell"
                    | "read"
                    | "search"
                    | "edit"
                    | "write"
                    | "delegate"
                    | "skill"
                    | "mcp"
                    | "other"
            )
        })
    }) {
        return Err(invalid("The tool category is invalid."));
    }
    if tool
        .get("provider_executed")
        .is_some_and(|flag| !flag.is_boolean())
    {
        return Err(invalid("The execution flag must be a Boolean."));
    }
    if tool.get("paths").is_some_and(|paths| {
        !paths.as_array().is_some_and(|items| {
            items.iter().all(|item| {
                item.as_str()
                    .is_some_and(crate::filters::is_repository_path)
            })
        })
    }) {
        return Err(invalid(
            "Recorded paths must be repository-relative strings.",
        ));
    }
    if kind == "tool_call" {
        if tool.get("result").is_some() {
            return Err(invalid("A call cannot contain a result."));
        }
        Ok(())
    } else {
        validate_result(
            tool.get("result")
                .ok_or_else(|| invalid("A result requires a status."))?,
        )
    }
}

fn validate_result(result: &Value) -> Result<(), SourceError> {
    closed(result, &["status", "content", "outcome", "output_bytes"])?;
    if !result
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| matches!(status, "completed" | "error" | "cancelled" | "unknown"))
    {
        return Err(invalid("The result status is invalid."));
    }
    if result
        .get("output_bytes")
        .is_some_and(|size| size.as_u64().is_none())
    {
        return Err(invalid(
            "The result output size must be a nonnegative integer.",
        ));
    }
    if let Some(outcome) = result.get("outcome") {
        closed(
            outcome,
            &[
                "kind",
                "state",
                "exit",
                "signal",
                "timed_out",
                "shell_id",
                "error",
                "session_id",
            ],
        )?;
        for (key, fact) in outcome.as_object().expect("outcome was checked") {
            let valid = match key.as_str() {
                "timed_out" | "error" => fact.is_boolean(),
                "exit" => fact.as_i64().is_some(),
                "signal" => fact.is_string() || fact.as_i64().is_some(),
                _ => fact.is_string(),
            };
            if !valid {
                return Err(invalid("A native outcome fact has an invalid type."));
            }
        }
    }
    Ok(())
}

fn closed(value: &Value, fields: &[&str]) -> Result<(), SourceError> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("A public record must be an object."))?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(invalid("A public record contains an unknown field."));
    }
    Ok(())
}

fn required_text<'a>(value: &'a Value, key: &str) -> Result<&'a str, SourceError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| invalid("A required public identity or name is unavailable."))
}
