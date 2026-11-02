//! Main 64 retains result progress but has no public-event state.

use super::*;

pub(super) fn convert(bytes: &[u8]) -> anyhow::Result<NativeFileCursor> {
    anyhow::ensure!(
        bytes.len() <= 32 * 1024 * 1024,
        NativeFileReadError::InvalidCursor
    );
    let mut value: Value = serde_json::from_slice(bytes)?;
    let fields = [
        "revision",
        "agent",
        "session_id",
        "source_format",
        "anchor",
        "baseline_offset",
        "start_offset",
        "next_record_index",
        "framer",
        "adapter",
        "requests",
        "request_ids",
        "request_bytes",
        "history_complete",
        "pending_results",
    ];
    anyhow::ensure!(
        value["revision"] == 1
            && value
                .as_object()
                .is_some_and(|object| object.len() == fields.len()
                    && fields.iter().all(|field| object.contains_key(*field))),
        NativeFileReadError::InvalidCursor
    );
    let agent = value["agent"]
        .as_str()
        .ok_or(NativeFileReadError::InvalidCursor)?
        .to_owned();
    let session = value["session_id"]
        .as_str()
        .ok_or(NativeFileReadError::InvalidCursor)?
        .to_owned();
    let (adapter, format) = match agent.as_str() {
        "claude" => ("Claude", "claude_jsonl"),
        "codex" => ("Codex", "codex_rollout_jsonl"),
        "pi" => ("Pi", "pi_v3_jsonl"),
        _ => anyhow::bail!(NativeFileReadError::InvalidCursor),
    };
    anyhow::ensure!(
        value["source_format"] == format
            && value["adapter"]
                .as_object()
                .is_some_and(|object| object.len() == 1 && object.contains_key(adapter)),
        NativeFileReadError::InvalidCursor
    );
    let wrapper_fields: &[&str] = match agent.as_str() {
        "claude" => &["parser", "retired_calls", "records", "last", "root"],
        "codex" => &[
            "parser",
            "retired_calls",
            "replaced_calls",
            "completed_ids",
            "root",
            "paginated",
            "ordinal",
            "suffix_only",
        ],
        "pi" => &[
            "parser",
            "retired_calls",
            "records",
            "last",
            "root",
            "suffix_only",
        ],
        _ => unreachable!(),
    };
    anyhow::ensure!(
        value["adapter"][adapter]
            .as_object()
            .is_some_and(|object| object.len() == wrapper_fields.len()
                && wrapper_fields
                    .iter()
                    .all(|field| object.contains_key(*field))),
        NativeFileReadError::InvalidCursor
    );
    for filter in [
        "retired_calls",
        "records",
        "replaced_calls",
        "completed_ids",
    ] {
        if let Some(bits) = value["adapter"][adapter].get(filter) {
            validate_filter(bits)?;
        }
    }
    validate_filter(&value["request_ids"])?;
    let parser = value["adapter"][adapter]["parser"]
        .as_object_mut()
        .ok_or(NativeFileReadError::InvalidCursor)?;
    let parser_fields: &[&str] = match agent.as_str() {
        "claude" => &[
            "best_priceable",
            "context",
            "context_window",
            "context_window_source",
            "first_model",
            "last_seen_model",
            "max_usage_by_message_id",
            "native_call_bytes",
            "native_calls",
            "ordinal",
            "pending_commands",
            "record_offset",
            "retained_root",
            "scope",
            "seen_uuids",
            "session_id",
            "skill_base_names",
            "source_format",
            "threads",
            "tool_identities",
            "user_message_root",
        ],
        "codex" => &[
            "agent_path",
            "cache_write_tokens_available",
            "completed",
            "context",
            "context_window",
            "current_model",
            "current_provider",
            "current_speed",
            "current_thinking_mode",
            "effort_seen",
            "fork_attribution_incomplete",
            "fork_parent_id",
            "fork_parent_seen",
            "fork_replay_timestamp",
            "model",
            "native_call_bytes",
            "native_calls",
            "owned_usage_seen",
            "ownership",
            "pending_bytes",
            "pending_owned_start",
            "pending_rows",
            "previous_boundary_ts",
            "previous_event_was_boundary",
            "previous_usage_key",
            "recent_cross_format_usage",
            "record_offset",
            "scope",
            "session_id",
            "source_format",
            "started_at_ms",
            "tool_identities",
        ],
        "pi" => &[
            "admission",
            "admission_checked",
            "branch_parents",
            "branched_tree",
            "cache_write_tokens_available",
            "current_model",
            "current_provider",
            "current_thinking_mode",
            "fork_attribution_incomplete",
            "fork_header_present",
            "fork_start_ms",
            "legacy_entry_ids",
            "legacy_migration_incomplete",
            "model",
            "native",
            "native_session_id",
            "native_tool_bytes",
            "native_tool_calls",
            "native_tool_results",
            "native_tool_results_enabled",
            "native_tool_results_incomplete",
            "policy_by_id",
            "provider_hints",
            "publication_root",
            "record_offset",
            "scope",
            "scope_enabled",
            "session_version",
            "started_at_ms",
            "startup_context",
            "subagent_calls",
            "subagent_incomplete",
            "threads",
        ],
        _ => unreachable!(),
    };
    anyhow::ensure!(
        parser.len() == parser_fields.len()
            && parser_fields
                .iter()
                .all(|field| parser.contains_key(*field)),
        NativeFileReadError::InvalidCursor
    );
    let session_field = if agent == "pi" {
        "native_session_id"
    } else {
        "session_id"
    };
    anyhow::ensure!(
        parser.get(session_field).and_then(Value::as_str) == Some(session.as_str()),
        NativeFileReadError::InvalidCursor
    );
    anyhow::ensure!(
        agent == "pi" || parser.get("source_format").and_then(Value::as_str) == Some(format),
        NativeFileReadError::InvalidCursor
    );
    if agent == "claude" {
        let calls = parser
            .get("native_calls")
            .and_then(Value::as_object)
            .ok_or(NativeFileReadError::InvalidCursor)?;
        anyhow::ensure!(
            calls.len() <= 4096
                && calls.values().all(|call| call.is_null()
                    || call.as_object().is_some_and(|object| object.len() == 4
                        && ["record_id", "name", "arguments", "retained_bytes"]
                            .iter()
                            .all(|field| object.contains_key(*field)))),
            NativeFileReadError::InvalidCursor
        );
    } else if agent == "codex" {
        let encoded = parser
            .get("native_calls")
            .and_then(Value::as_str)
            .ok_or(NativeFileReadError::InvalidCursor)?;
        let mut calls: Value = serde_json::from_str(encoded)?;
        add_call_facts(&mut calls, true)?;
        parser.insert(
            "native_calls".into(),
            Value::String(serde_json::to_string(&calls)?),
        );
        for (field, initial) in [
            ("native_provider", Value::Null),
            (
                "native_request",
                serde_json::to_value(super::super::super::NativeRequestAttribution::default())?,
            ),
            ("native_settings", Value::Null),
            ("native_mode_contexts", serde_json::json!({})),
            ("native_completed_mode", Value::Null),
            ("native_turn_id", Value::Null),
            ("native_cwd", Value::Null),
        ] {
            anyhow::ensure!(
                !parser.contains_key(field),
                NativeFileReadError::InvalidCursor
            );
            parser.insert(field.into(), initial);
        }
    } else if agent == "pi" {
        add_call_facts(
            parser
                .get_mut("native_tool_calls")
                .ok_or(NativeFileReadError::InvalidCursor)?,
            false,
        )?;
    }
    let legacy: CursorV4 = serde_json::from_value(value)?;
    let mut cursor = legacy.convert_revision(1)?;
    if let Some(pending) = &cursor.pending_results {
        anyhow::ensure!(
            pending.position.record_start <= pending.position.record_end
                && pending.position.record_end <= cursor.offset(),
            NativeFileReadError::InvalidCursor
        );
        for encoded in &pending.observations_json {
            let result: ToolResultObservation = serde_json::from_str(encoded)?;
            anyhow::ensure!(
                result.session_id == cursor.session_id
                    && result.source_format == cursor.source_format,
                NativeFileReadError::InvalidCursor
            );
        }
    }
    cursor.history_complete = false;
    cursor.main_64_public_baseline_pending = true;
    Ok(cursor)
}

fn validate_filter(value: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        value.as_array().is_some_and(
            |bits| bits.len() == 131_072 && bits.iter().all(|word| word.as_u64().is_some())
        ),
        NativeFileReadError::InvalidCursor
    );
    Ok(())
}

fn add_call_facts(calls: &mut Value, nullable: bool) -> anyhow::Result<()> {
    let calls = calls
        .as_object_mut()
        .ok_or(NativeFileReadError::InvalidCursor)?;
    anyhow::ensure!(calls.len() <= 4096, NativeFileReadError::InvalidCursor);
    for call in calls.values_mut() {
        if nullable && call.is_null() {
            continue;
        }
        let call = call
            .as_object_mut()
            .ok_or(NativeFileReadError::InvalidCursor)?;
        let fields: &[&str] = if nullable {
            &["name", "arguments", "retained_bytes"]
        } else {
            &["record_id", "name", "arguments"]
        };
        anyhow::ensure!(
            call.len() == fields.len() && fields.iter().all(|field| call.contains_key(*field)),
            NativeFileReadError::InvalidCursor
        );
        call.insert(
            "facts".into(),
            serde_json::to_value(super::super::super::NativeToolFacts::default())?,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
