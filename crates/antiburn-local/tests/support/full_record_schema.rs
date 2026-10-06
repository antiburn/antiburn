use serde_json::Value;
use std::collections::BTreeSet;

pub fn validate(artifact: &Value) -> Result<(), String> {
    if artifact["schema_version"] != 3 || artifact["record_count"] != 240 {
        return Err("invalid record schema or count".into());
    }
    let records = artifact["cases"].as_array().ok_or("missing case records")?;
    if records.len() != 240 {
        return Err("expected 240 materialized records".into());
    }
    let mut ids = BTreeSet::new();
    let mut heldout = 0;
    for record in records {
        let id = record["id"].as_str().ok_or("missing ID")?;
        if !ids.insert(id) {
            return Err(format!("{id}: duplicate ID"));
        }
        if record["split"] == "HELDOUT" {
            heldout += 1;
        }
        for key in ["family", "source_format", "source_pointer"] {
            if record[key].as_str().is_none() {
                return Err(format!("{id}: missing {key}"));
            }
        }
        for key in ["text", "source", "scope", "provenance"] {
            if record["instruction"][key].as_str().is_none() {
                return Err(format!("{id}: missing instruction {key}"));
            }
        }
        let reference = record["instruction"]["snapshot_text"]
            .as_str()
            .ok_or("missing reference snapshot")?;
        let range = &record["instruction"]["expected_authored_rule_range"];
        let start = usize::try_from(range["start"].as_u64().ok_or("missing reference start")?)
            .map_err(|error| error.to_string())?;
        let end = usize::try_from(range["end"].as_u64().ok_or("missing reference end")?)
            .map_err(|error| error.to_string())?;
        if reference.get(start..end) != record["instruction"]["text"].as_str() {
            return Err(format!(
                "{id}: reference citation range does not bind authored instruction"
            ));
        }
        for key in [
            "trigger",
            "exception",
            "completion_boundary",
            "lifecycle",
            "stage_budgets",
        ] {
            if !record[key].is_object() {
                return Err(format!("{id}: missing typed {key}"));
            }
        }
        let state = record["expected"]["state"]
            .as_str()
            .ok_or_else(|| format!("{id}: expected state must be typed, not a Boolean"))?;
        if !["finding", "no_finding", "pending", "conditional"].contains(&state) {
            return Err(format!("{id}: unknown expected state"));
        }
        if record["stage_budgets"]["provider_calls"]["max"] != 0 {
            return Err(format!("{id}: live calls are forbidden"));
        }
        if record["unavailable_private_labels"]["state"] != "unavailable"
            || record["unavailable_private_labels"]["human_confirmed"] != false
        {
            return Err(format!(
                "{id}: synthetic records cannot claim private human labels"
            ));
        }
        let events = record["events"]
            .as_array()
            .ok_or_else(|| format!("{id}: missing event expectations"))?;
        let mut event_ids = BTreeSet::new();
        for event in events {
            let event_id = event["event_id"].as_str().ok_or("missing event ID")?;
            if !event_ids.insert(event_id) {
                return Err(format!("{id}: duplicate event identity"));
            }
            if let Some(selected) = event["expected_selected"].as_object() {
                let field = selected["field"].as_str().ok_or("missing selected field")?;
                if ![
                    "AssistantMessage",
                    "BashCommandInput",
                    "FileEditPath",
                    "ReadFilePath",
                    "SearchFilesQuery",
                    "OtherToolInput",
                ]
                .contains(&field)
                    || !selected["text"].is_string()
                {
                    return Err(format!("{id}: excluded or invalid selected evidence"));
                }
            } else if !event["expected_selected"].is_null() {
                return Err(format!("{id}: unavailable evidence must not be Boolean"));
            }
        }
        let citations = record["expected"]["finding_citations"]
            .as_array()
            .ok_or("missing expected citations")?;
        if (state == "finding") != !citations.is_empty() {
            return Err(format!("{id}: finding must have independent citations"));
        }
        for citation in citations {
            if !events.iter().any(|event| {
                event["event_id"] == *citation && event["expected_selected"].is_object()
            }) {
                return Err(format!(
                    "{id}: citation does not bind selected authored evidence"
                ));
            }
        }
        if record["forbidden_claims"]
            .as_array()
            .is_none_or(|claims| claims.len() < 8)
        {
            return Err(format!("{id}: missing evidence claim boundaries"));
        }
    }
    if heldout != 48 {
        return Err("held-out membership count changed".into());
    }
    Ok(())
}
