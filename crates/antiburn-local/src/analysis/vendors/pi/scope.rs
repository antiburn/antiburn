//! Optional Pi extension shapes. Journal records do not identify loaded producers.

use std::collections::{HashMap, HashSet};

use crate::checks::ignored_instructions::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::SourceFormat;
use crate::analysis::framing::PartialReason;
use crate::analysis::interface::{ContentKind, ContentPart, NormalizedRecord, RecordSink};
use crate::analysis::jev::JevInputField;
use crate::analysis::jev_evidence::{
    JevAnswerSelection, JevNativeFieldContainer, JevNativeFieldRange, JevPlanContentStatus,
    JevPlanReference, JevPlanStatus, JevQuestionOption, JevScopeEvidenceProvenance,
    JevScopeEvidenceRole, JevScopeEvidenceSource, JevUserAnswer, JevUserAnswerOrigin,
    JevUserAnswerStatus,
};

const OFFICIAL: &str = "pi-mono:b2602be77cb7b0de45dd616407fd210daa48aa75:0.85.1";
const ASK_USER: &str = "edlsh/pi-ask-user:adcc7b2ee22bb290f9d6693efe6c0d0cd1273280:0.16.0";
const MAX_ROWS: usize = 50_000;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub(super) struct PiScopeState {
    rows: HashMap<String, ScopeRow>,
    bytes: usize,
    incomplete: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ScopeRow {
    parent: Option<String>,
    complete: bool,
    // JSON strings keep the resume snapshot compatible with postcard.
    calls: Vec<(String, String, String, u32)>,
    plan_state: Option<String>,
    assistant_text: Option<String>,
    assistant_bindings: Vec<JevNativeFieldRange>,
    todo_list: Option<String>,
}

impl PiScopeState {
    pub(super) fn incomplete(&self) -> bool {
        self.incomplete
    }

    fn ancestors<'a>(
        &'a self,
        value: &'a Value,
        cancel: &dyn Fn() -> bool,
    ) -> anyhow::Result<Option<Vec<(&'a str, &'a ScopeRow)>>> {
        let mut path = Vec::new();
        let mut visited = HashSet::new();
        if let Some(id) = value["id"].as_str() {
            visited.insert(id);
        }
        if !matches!(value.get("parentId"), Some(Value::Null | Value::String(_))) {
            return Ok(None);
        }
        let mut parent = value["parentId"].as_str();
        while let Some(id) = parent {
            anyhow::ensure!(!cancel(), "Pi scope ancestry read was cancelled");
            if path.len() >= MAX_ROWS || !visited.insert(id) {
                return Ok(None);
            }
            let Some(row) = self.rows.get(id) else {
                return Ok(None);
            };
            path.push((id, row));
            parent = row.parent.as_deref();
        }
        Ok(Some(path))
    }

    pub(super) fn observe(
        &mut self,
        value: &Value,
        sink: &mut dyn RecordSink,
        cancel: &dyn Fn() -> bool,
    ) -> anyhow::Result<Vec<ContentPart>> {
        let message = &value["message"];
        let needs_ancestry = (message["role"] == "toolResult"
            && matches!(
                message["toolName"].as_str(),
                Some("question" | "questionnaire" | "ask_user")
            ))
            || matches!(
                value["customType"].as_str(),
                Some("plan-mode" | "plan-mode-execute")
            );
        let ancestry = if needs_ancestry {
            self.ancestors(value, cancel)?
        } else {
            Some(Vec::new())
        };
        let lineage_complete = ancestry.is_some();
        if !lineage_complete {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
        }
        let path = ancestry.unwrap_or_default();
        let complete =
            lineage_complete && record_complete(value) && path.iter().all(|(_, row)| row.complete);
        let mut answers = Vec::new();
        let mut plans = Vec::new();
        if message["role"] == "toolResult" {
            let id = message["toolCallId"].as_str();
            let name = message["toolName"].as_str();
            let matches: Vec<_> = path
                .iter()
                .flat_map(|(record_id, row)| row.calls.iter().map(move |call| (*record_id, call)))
                .filter(|(_, (call_id, _, _, _))| Some(call_id.as_str()) == id)
                .collect();
            if let [(record_id, (_, call_name, args, index))] = matches.as_slice()
                && Some(call_name.as_str()) == name
                && let Ok(args) = serde_json::from_str::<Value>(args)
            {
                answers = question_results(value, call_name, &args);
                for answer in &mut answers {
                    answer.source.truncated |= !complete;
                    if !complete {
                        answer.status = JevUserAnswerStatus::Unknown;
                    }
                    answer.source.bindings.extend(native_strings(
                        &args,
                        &format!("/message/content/{index}/arguments"),
                        JevInputField::UserAnswer,
                        Some(record_id),
                    ));
                }
            }
        }
        if value["type"] == "custom" && value["customType"] == "plan-mode" {
            if valid_plan_state(&value["data"])
                && value["data"]["todos"]
                    .as_array()
                    .is_some_and(|todos| !todos.is_empty())
            {
                let proposal = complete
                    .then(|| matched_proposal(&path, &value["data"]["todos"]))
                    .flatten();
                let mut p = plan(
                    value,
                    proposal.map(|(id, _)| id.into()),
                    proposal.map(|(_, t)| t.into()),
                    JevPlanStatus::Proposed,
                );
                if value["data"]["executing"] == true {
                    p.status = JevPlanStatus::Unknown;
                }
                p.source.truncated |= !complete;
                if let Some((id, _)) = proposal
                    && let Some((_, row)) = path.iter().find(|(record_id, _)| *record_id == id)
                {
                    p.source.bindings.extend(row.assistant_bindings.clone());
                }
                if !complete {
                    p.status = JevPlanStatus::Unknown;
                }
                plans.push(p);
            }
        } else if value["type"] == "custom_message" && value["customType"] == "plan-mode-execute" {
            let mut p = execution_plan(value, &path);
            p.source.truncated |= !complete;
            plans.push(p);
        }
        if !lineage_complete {
            self.incomplete = true;
        }
        let payload = if !answers.is_empty() || !plans.is_empty() {
            Some(
                serde_json::to_string(if message["role"] == "toolResult" {
                    &message["details"]
                } else {
                    value
                })
                .expect("JSON value serializes"),
            )
        } else {
            None
        };
        let mut parts = Vec::new();
        if let Some(payload) = payload {
            let kind = ContentKind::ToolResult;
            let part = ContentPart::new(kind, payload).with_tool_identity(
                message["toolName"].as_str().map(str::to_owned),
                message["toolCallId"].as_str().map(str::to_owned),
            );
            match part.with_scope_evidence(std::mem::take(&mut answers), plans) {
                Ok(part) => parts.push(part),
                Err(_) => {
                    self.incomplete = true;
                    sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
                }
            }
        }
        let Some(id) = value["id"].as_str() else {
            return Ok(parts);
        };
        let calls: Vec<(String, String, String, u32)> = message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, b)| message["role"] == "assistant" && b["type"] == "toolCall")
            .filter_map(|(index, b)| {
                let name = b["name"].as_str()?;
                Some((
                    b["id"].as_str()?.to_owned(),
                    name.to_owned(),
                    if matches!(name, "question" | "questionnaire" | "ask_user") {
                        serde_json::to_string(&b["arguments"]).ok()?
                    } else {
                        "null".into()
                    },
                    u32::try_from(index).ok()?,
                ))
            })
            .collect();
        for (call_id, name, args, index) in &calls {
            let args: Value = serde_json::from_str(args).expect("stored call JSON");
            let pending = serde_json::json!({"id": value["id"], "message": {"role": "toolResult", "toolCallId": call_id, "toolName": name}});
            let mut answers = question_results(&pending, name, &args);
            for a in &mut answers {
                a.status = JevUserAnswerStatus::Pending;
                a.source.role = JevScopeEvidenceRole::Assistant;
                a.source.truncated = !record_complete(value);
                a.source.bindings = native_strings(
                    &args,
                    &format!("/message/content/{index}/arguments"),
                    JevInputField::UserAnswer,
                    Some(id),
                );
            }
            if !answers.is_empty() {
                let part = ContentPart::new(ContentKind::ToolInput, args.to_string())
                    .with_tool_identity(Some(name.clone()), Some(call_id.clone()));
                match part.with_scope_evidence(answers, Vec::new()) {
                    Ok(part) => parts.push(part),
                    Err(_) => {
                        self.incomplete = true;
                        sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
                    }
                }
            }
        }
        let assistant_text = (message["role"] == "assistant")
            .then(|| text(&message["content"]))
            .filter(|t| t.to_ascii_lowercase().contains("plan:"));
        let assistant_bindings = if assistant_text.is_some() {
            text_bindings(
                &message["content"],
                "/message/content",
                JevInputField::PlanReference,
                Some(id),
            )
        } else {
            Vec::new()
        };
        let row = ScopeRow {
            parent: value["parentId"].as_str().map(str::to_owned),
            complete: record_complete(value),
            calls,
            plan_state: (value["type"] == "custom" && value["customType"] == "plan-mode")
                .then(|| value["data"].to_string()),
            assistant_text,
            assistant_bindings,
            todo_list: (value["type"] == "custom_message"
                && value["customType"] == "plan-todo-list")
                .then(|| text(&value["content"])),
        };
        let bytes = id.len()
            + serde_json::to_vec(&row)
                .expect("scope row serializes")
                .len();
        if self.rows.len() >= MAX_ROWS || self.bytes.saturating_add(bytes) > MAX_BYTES {
            self.incomplete = true;
        } else {
            self.bytes += bytes;
            self.rows.insert(id.to_owned(), row);
        }
        Ok(parts)
    }
}

fn text_bindings(
    content: &Value,
    pointer: &str,
    field: JevInputField,
    id: Option<&str>,
) -> Vec<JevNativeFieldRange> {
    if content.is_string() {
        return native_strings(content, pointer, field, id);
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, block)| block["type"] == "text")
        .flat_map(|(i, block)| {
            native_strings(&block["text"], &format!("{pointer}/{i}/text"), field, id)
        })
        .collect()
}

fn text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn source(value: &Value, pin: &str) -> JevScopeEvidenceSource {
    let (native, pointer, field) = if value["message"]["role"] == "toolResult" {
        (
            &value["message"]["details"],
            "/message/details",
            JevInputField::UserAnswer,
        )
    } else if value["type"] == "custom" {
        (&value["data"], "/data", JevInputField::PlanReference)
    } else {
        (&value["content"], "/content", JevInputField::PlanReference)
    };
    JevScopeEvidenceSource {
        source_format: SourceFormat::PiV3Jsonl,
        role: if value["message"]["role"] == "toolResult" {
            JevScopeEvidenceRole::Tool
        } else {
            JevScopeEvidenceRole::Unknown
        },
        native_record_id: value["id"].as_str().map(str::to_owned),
        call_id: value["message"]["toolCallId"].as_str().map(str::to_owned),
        question_id: None,
        order: 0,
        acceptance_order: None,
        provenance: JevScopeEvidenceProvenance::Unknown,
        producer_revision: pin.into(),
        normalization_revision: 1,
        bindings: native_strings(native, pointer, field, value["id"].as_str()),
        truncated: !record_complete(value),
    }
}

fn record_complete(value: &Value) -> bool {
    fn intact(value: &Value) -> bool {
        value["truncated"] != true && value["incomplete"] != true && value["complete"] != false
    }
    intact(value)
        && intact(&value["message"])
        && intact(&value["data"])
        && intact(&value["details"])
        && intact(&value["message"]["details"])
        && !matches!(
            value["message"]["stopReason"].as_str(),
            Some("aborted" | "error" | "length")
        )
        && [&value["content"], &value["message"]["content"]]
            .into_iter()
            .all(|content| content.as_array().into_iter().flatten().all(intact))
}

fn native_strings(
    value: &Value,
    pointer: &str,
    field: JevInputField,
    native_record_id: Option<&str>,
) -> Vec<JevNativeFieldRange> {
    let mut bindings = Vec::new();
    let mut pending = vec![(value, pointer.to_owned())];
    while let Some((value, pointer)) = pending.pop() {
        match value {
            Value::String(text) => bindings.push(JevNativeFieldRange {
                native_record_id: native_record_id.map(str::to_owned),
                field,
                container: JevNativeFieldContainer::Record,
                pointer,
                start: 0,
                end: text.len(),
            }),
            Value::Array(values) => pending.extend(
                values
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (v, format!("{pointer}/{i}"))),
            ),
            Value::Object(values) => pending.extend(values.iter().map(|(k, v)| {
                (
                    v,
                    format!("{pointer}/{}", k.replace('~', "~0").replace('/', "~1")),
                )
            })),
            _ => {}
        }
    }
    bindings
}

fn options(value: &Value, label_key: &str) -> Option<Vec<JevQuestionOption>> {
    value
        .as_array()?
        .iter()
        .map(|o| {
            Some(JevQuestionOption {
                id: None,
                value: o["value"].as_str().map(str::to_owned),
                label: o.as_str().or_else(|| o[label_key].as_str())?.to_owned(),
                description: o["description"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

fn answer(
    value: &Value,
    pin: &str,
    prompt: &str,
    options: Vec<JevQuestionOption>,
) -> JevUserAnswer {
    JevUserAnswer {
        source: source(value, pin),
        prompt: prompt.into(),
        header: None,
        context: None,
        comment: None,
        options,
        multi_select: Some(false),
        selections: Vec::new(),
        free_text: None,
        status: JevUserAnswerStatus::Unknown,
        origin: JevUserAnswerOrigin::UnknownOrigin,
    }
}

fn selection(value: &str, label: &str, custom: bool, index: Option<u32>) -> JevAnswerSelection {
    JevAnswerSelection {
        option_id: None,
        option_index: index,
        value: Some(value.into()),
        label: Some(label.into()),
        custom: Some(custom),
    }
}

fn question_results(value: &Value, name: &str, args: &Value) -> Vec<JevUserAnswer> {
    let details = &value["message"]["details"];
    let output = text(&value["message"]["content"]);
    let mut answers = match name {
        "question" => single_question(value, args, details, &output)
            .into_iter()
            .collect(),
        "questionnaire" => questionnaire(value, args, details, &output),
        "ask_user" => ask_user(value, args, details),
        _ => Vec::new(),
    };
    for answer in &mut answers {
        let content = &value["message"]["content"];
        if content.is_string() {
            answer.source.bindings.extend(native_strings(
                content,
                "/message/content",
                JevInputField::UserAnswer,
                value["id"].as_str(),
            ));
        } else if let Some(blocks) = content.as_array() {
            for (index, block) in blocks
                .iter()
                .enumerate()
                .filter(|(_, block)| block["type"] == "text")
            {
                answer.source.bindings.extend(native_strings(
                    &block["text"],
                    &format!("/message/content/{index}/text"),
                    JevInputField::UserAnswer,
                    value["id"].as_str(),
                ));
            }
        }
        if value["message"]["isError"] == true || answer.source.truncated {
            answer.status = JevUserAnswerStatus::Unknown;
        }
    }
    answers
}

fn single_question(
    value: &Value,
    args: &Value,
    details: &Value,
    output: &str,
) -> Option<JevUserAnswer> {
    let prompt = args["question"].as_str()?;
    let opts = options(&args["options"], "label")?;
    let mut a = answer(value, OFFICIAL, prompt, opts);
    if !details.is_null() {
        details.get("answer")?;
        if details["question"] != prompt
            || details["options"]
                != Value::Array(
                    a.options
                        .iter()
                        .map(|o| Value::String(o.label.clone()))
                        .collect(),
                )
        {
            return None;
        }
        if details["answer"].is_null() {
            a.status = if output == "User cancelled the selection" {
                JevUserAnswerStatus::Cancelled
            } else {
                JevUserAnswerStatus::Unknown
            };
            return Some(a);
        }
        let response = details["answer"].as_str()?;
        let custom = details["wasCustom"].as_bool()?;
        apply_official_selection(&mut a, response, response, custom, None)?;
    } else if output == "User cancelled the selection" {
        a.status = JevUserAnswerStatus::Cancelled;
    } else if let Some(response) = output.strip_prefix("User wrote: ") {
        apply_official_selection(&mut a, response, response, true, None)?;
    } else if let Some(response) = output.strip_prefix("User selected: ") {
        let (index, label) = response.split_once(". ")?;
        apply_official_selection(&mut a, label, label, false, Some(index.parse().ok()?))?;
    }
    Some(a)
}

fn apply_official_selection(
    a: &mut JevUserAnswer,
    value: &str,
    label: &str,
    custom: bool,
    index: Option<u32>,
) -> Option<()> {
    if !custom {
        let matches: Vec<_> = a
            .options
            .iter()
            .enumerate()
            .filter(|(_, o)| o.label == label && o.value.as_deref().unwrap_or(&o.label) == value)
            .map(|(i, _)| i)
            .collect();
        if matches.is_empty() {
            return None;
        }
        let selected = if let Some(index) = index {
            let selected = usize::try_from(index.checked_sub(1)?).ok()?;
            if !matches.contains(&selected) {
                return None;
            }
            Some(selected)
        } else if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        };
        a.selections.push(selection(
            value,
            label,
            false,
            selected.and_then(|n| u32::try_from(n).ok()),
        ));
    } else {
        a.selections.push(selection(value, label, true, None));
        a.free_text = Some(value.into());
    }
    a.status = JevUserAnswerStatus::Submitted;
    Some(())
}

fn questionnaire(value: &Value, args: &Value, details: &Value, output: &str) -> Vec<JevUserAnswer> {
    let Some(questions) = args["questions"].as_array() else {
        return Vec::new();
    };
    let cancelled = details["cancelled"].as_bool();
    let mut seen = std::collections::HashSet::new();
    let labels: Vec<_> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            q["label"]
                .as_str()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Q{}", i + 1))
        })
        .collect();
    let text_unambiguous = labels
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        == questions.len()
        && (questions.len() == 1 || output.lines().count() == questions.len());
    let mut result = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        let Some(id) = q["id"].as_str() else {
            return Vec::new();
        };
        if !seen.insert(id) {
            return Vec::new();
        }
        let Some(prompt) = q["prompt"].as_str() else {
            return Vec::new();
        };
        let Some(opts) = options(&q["options"], "label") else {
            return Vec::new();
        };
        let mut a = answer(value, OFFICIAL, prompt, opts);
        a.header = Some(labels[i].clone());
        a.source.question_id = Some(id.into());
        a.source.order = u32::try_from(i).expect("bounded record question count");
        let recorded = details["questions"]
            .as_array()
            .and_then(|qs| qs.iter().find(|rq| rq["id"] == id));
        let matches =
            recorded.is_some_and(|rq| rq["prompt"] == q["prompt"] && rq["options"] == q["options"]);
        if (cancelled == Some(true) && matches && !output.starts_with("Error:"))
            || (details.is_null() && output == "User cancelled the questionnaire")
        {
            a.status = JevUserAnswerStatus::Cancelled;
        } else if details.is_null() && text_unambiguous {
            let default_label = format!("Q{}", i + 1);
            let label = q["label"]
                .as_str()
                .filter(|l| !l.is_empty())
                .unwrap_or(&default_label);
            let prefix = format!("{label}: user ");
            let lines: Vec<_> = if questions.len() == 1 {
                output.strip_prefix(&prefix).into_iter().collect()
            } else {
                output
                    .lines()
                    .filter_map(|l| l.strip_prefix(&prefix))
                    .collect()
            };
            if let [line] = lines.as_slice() {
                if let Some(response) = line.strip_prefix("wrote: ") {
                    let _ = apply_official_selection(&mut a, response, response, true, None);
                } else if let Some(response) = line.strip_prefix("selected: ")
                    && let Some((index, label)) = response.split_once(". ")
                    && let Ok(index) = index.parse::<u32>()
                    && let Some(option_index) =
                        index.checked_sub(1).and_then(|n| usize::try_from(n).ok())
                    && let Some(option) = a.options.get(option_index)
                {
                    let option_value = option.value.clone().unwrap_or_else(|| option.label.clone());
                    let _ =
                        apply_official_selection(&mut a, &option_value, label, false, Some(index));
                }
            }
        } else if cancelled == Some(false) && matches {
            let responses: Vec<_> = details["answers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["id"] == id)
                .collect();
            if let [r] = responses.as_slice()
                && let (Some(v), Some(l), Some(custom)) = (
                    r["value"].as_str(),
                    r["label"].as_str(),
                    r["wasCustom"].as_bool(),
                )
            {
                let index = r["index"].as_u64().and_then(|n| u32::try_from(n).ok());
                let _ = apply_official_selection(&mut a, v, l, custom, index);
            }
        }
        if cancelled == Some(true) && matches && !output.starts_with("Error:") {
            let responses: Vec<_> = details["answers"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|r| r["id"] == id)
                .collect();
            if let [r] = responses.as_slice()
                && let (Some(v), Some(l), Some(custom)) = (
                    r["value"].as_str(),
                    r["label"].as_str(),
                    r["wasCustom"].as_bool(),
                )
            {
                let index = r["index"].as_u64().and_then(|n| u32::try_from(n).ok());
                let _ = apply_official_selection(&mut a, v, l, custom, index);
                a.status = JevUserAnswerStatus::Cancelled;
            }
        }
        result.push(a);
    }
    result
}

fn ask_user(value: &Value, args: &Value, details: &Value) -> Vec<JevUserAnswer> {
    let batch = args["questions"].as_array();
    let questions: Vec<_> = batch.map_or_else(|| vec![args], |qs| qs.iter().collect());
    if batch.is_some() && !matches!(questions.len(), 2..=4) {
        return Vec::new();
    }
    let mut result = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        let Some(prompt) = q["question"].as_str() else {
            return Vec::new();
        };
        let d = if batch.is_some() {
            &details["questions"][i]
        } else {
            details
        };
        let opts = options(&d["options"], "title").unwrap_or_else(|| ask_options(&q["options"]));
        let mut a = answer(value, ASK_USER, prompt, opts);
        a.context = q["context"].as_str().map(str::to_owned);
        a.comment = if batch.is_some() {
            details["answers"][i]["response"]["comment"].as_str()
        } else {
            details["response"]["comment"].as_str()
        }
        .map(str::to_owned);
        a.source.order = u32::try_from(i).expect("bounded question count");
        a.multi_select = Some(q["allowMultiple"].as_bool().unwrap_or(false));
        if details["cancelled"] == true {
            a.status = JevUserAnswerStatus::Cancelled;
        } else if d["question"] == prompt
            && details["cancelled"] == false
            && options(&d["options"], "title") == Some(ask_options(&q["options"]))
            && (batch.is_none() || details["kind"] == "batch")
        {
            let response = if batch.is_some() {
                &details["answers"][i]["response"]
            } else {
                &details["response"]
            };
            if batch.is_some() && details["answers"][i]["status"] != "answered" {
                a.status = if details["answers"][i]["status"] == "skipped" {
                    JevUserAnswerStatus::Skipped
                } else {
                    JevUserAnswerStatus::Unknown
                };
            } else if response["kind"] == "freeform"
                && let Some(t) = response["text"].as_str()
            {
                a.free_text = Some(t.into());
                a.status = JevUserAnswerStatus::Submitted;
            } else if response["kind"] == "selection"
                && let Some(selections) = response["selections"].as_array()
            {
                let valid = !selections.is_empty()
                    && (a.multi_select == Some(true) || selections.len() == 1)
                    && selections.iter().all(|s| {
                        s.as_str()
                            .is_some_and(|s| a.options.iter().any(|o| o.label == s))
                    });
                if valid {
                    a.selections = selections
                        .iter()
                        .filter_map(|s| {
                            let s = s.as_str()?;
                            let index = a.options.iter().position(|o| o.label == s)?;
                            Some(selection(s, s, false, u32::try_from(index).ok()))
                        })
                        .collect();
                    a.comment = response["comment"].as_str().map(str::to_owned);
                    a.status = JevUserAnswerStatus::Submitted;
                }
            } else {
                a.status = JevUserAnswerStatus::Pending;
            }
        }
        result.push(a);
    }
    result
}

fn ask_options(value: &Value) -> Vec<JevQuestionOption> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let title = match option {
                Value::String(t) => t.trim().to_owned(),
                Value::Number(_) | Value::Bool(_) => option.to_string(),
                Value::Object(_) => ["title", "label", "text", "value", "name", "option"]
                    .iter()
                    .find_map(|key| {
                        option[key]
                            .as_str()
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                    })?
                    .to_owned(),
                _ => return None,
            };
            if title.is_empty() {
                return None;
            }
            Some(JevQuestionOption {
                id: None,
                value: None,
                label: title,
                description: option["description"]
                    .as_str()
                    .filter(|d| !d.trim().is_empty())
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn valid_plan_state(data: &Value) -> bool {
    data["enabled"].is_boolean()
        && data["executing"].is_boolean()
        && data["todos"].as_array().is_some_and(|todos| {
            todos.iter().enumerate().all(|(i, t)| {
                t["step"].as_u64() == u64::try_from(i + 1).ok()
                    && t["text"].is_string()
                    && t["completed"].is_boolean()
            })
        })
}

fn plan(
    value: &Value,
    revision: Option<String>,
    text: Option<String>,
    status: JevPlanStatus,
) -> JevPlanReference {
    let digest = text.as_ref().map(|t| sha256_hex(t.as_bytes()));
    JevPlanReference {
        source: source(value, OFFICIAL),
        plan_id: Some("plan-mode".into()),
        path: None,
        revision,
        content_digest: digest,
        approved_revision: None,
        approved_content_digest: None,
        content_status: if text.is_some() {
            JevPlanContentStatus::Recorded
        } else {
            JevPlanContentStatus::Unresolved
        },
        text,
        feedback: None,
        status,
        origin: JevUserAnswerOrigin::UnknownOrigin,
    }
}

fn execution_plan(value: &Value, path: &[(&str, &ScopeRow)]) -> JevPlanReference {
    let mut p = plan(value, None, None, JevPlanStatus::Unknown);
    p.feedback = Some(text(&value["content"]));
    p.source.truncated |= path.iter().any(|(_, row)| !row.complete);
    if p.source.truncated {
        return p;
    }
    let states: Vec<_> = path
        .iter()
        .filter_map(|(id, r)| r.plan_state.as_ref().map(|s| (*id, s)))
        .take(2)
        .collect();
    let [(execute_id, execute), (proposed_id, proposed)] = states.as_slice() else {
        return p;
    };
    let (Ok(execute), Ok(proposed)) = (
        serde_json::from_str::<Value>(execute),
        serde_json::from_str::<Value>(proposed),
    ) else {
        return p;
    };
    if !valid_plan_state(&execute)
        || !valid_plan_state(&proposed)
        || execute["enabled"] != false
        || execute["executing"] != true
        || proposed["enabled"] != true
        || proposed["executing"] != false
        || execute["todos"] != proposed["todos"]
    {
        return p;
    }
    let Some(todos) = execute["todos"].as_array().filter(|t| !t.is_empty()) else {
        return p;
    };
    let remaining = todos
        .iter()
        .map(|t| {
            format!(
                "{}. {}",
                t["step"],
                t["text"].as_str().expect("validated todo")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let expected = format!(
        "Execute the plan.\n\nRemaining steps:\n{remaining}\n\nStart with: {}\nAfter completing a step, include a [DONE:n] tag in your response.",
        todos[0]["text"].as_str().expect("validated todo")
    );
    let list = format!(
        "**Plan Steps ({}):**\n\n{}",
        todos.len(),
        todos
            .iter()
            .enumerate()
            .map(|(i, t)| format!(
                "{}. ☐ {}",
                i + 1,
                t["text"].as_str().expect("validated todo")
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if value["display"] != true
        || text(&value["content"]) != expected
        || path.first().and_then(|(_, r)| r.todo_list.as_ref()) != Some(&list)
    {
        return p;
    }
    let proposal_index = path
        .iter()
        .position(|(id, _)| id == proposed_id)
        .expect("ancestor state exists");
    if let Some((id, proposal)) = matched_proposal(&path[proposal_index + 1..], &proposed["todos"])
    {
        p = plan(
            value,
            Some(id.into()),
            Some(proposal.into()),
            JevPlanStatus::Approved,
        );
        p.approved_revision = p.revision.clone();
        p.approved_content_digest = p.content_digest.clone();
        let row = path
            .iter()
            .find(|(record_id, _)| *record_id == id)
            .expect("matched proposal exists")
            .1;
        p.source.bindings.extend(row.assistant_bindings.clone());
    } else {
        p.revision = Some((*execute_id).into());
    }
    p.status = JevPlanStatus::Approved;
    p.origin = JevUserAnswerOrigin::Synthetic;
    p
}

fn matched_proposal<'a>(
    path: &[(&'a str, &'a ScopeRow)],
    todos: &Value,
) -> Option<(&'a str, &'a str)> {
    let (id, text) = path
        .iter()
        .find(|(_, row)| row.assistant_text.is_some())
        .filter(|(_, row)| row.complete)
        .and_then(|(id, row)| row.assistant_text.as_deref().map(|t| (*id, t)))?;
    let extracted = extract_todos(text)?;
    (Value::Array(extracted) == *todos).then_some((id, text))
}

fn extract_todos(text: &str) -> Option<Vec<Value>> {
    let lower = text.to_ascii_lowercase();
    let section = lower.match_indices("plan:").find_map(|(start, _)| {
        let suffix = &text[start + 5..];
        let suffix = suffix
            .strip_prefix("**")
            .or_else(|| suffix.strip_prefix('*'))
            .unwrap_or(suffix);
        let end = suffix.find('\n')?;
        suffix[..end]
            .chars()
            .all(char::is_whitespace)
            .then_some(&suffix[end + 1..])
    })?;
    let mut todos = Vec::new();
    for line in section.lines() {
        let line = line.trim_start();
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            continue;
        }
        let Some(rest) = line[digits..]
            .strip_prefix('.')
            .or_else(|| line[digits..].strip_prefix(')'))
        else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("**")
            .or_else(|| rest.strip_prefix('*'))
            .unwrap_or(rest);
        let raw = rest.split('*').next()?.trim();
        if raw.encode_utf16().count() <= 5 || raw.starts_with(['`', '/', '-']) {
            continue;
        }
        let mut cleaned = String::new();
        let mut remaining = raw;
        while let Some(start) = remaining.find('`') {
            cleaned.push_str(&remaining[..start]);
            let after = &remaining[start + 1..];
            if let Some(end) = after.find('`').filter(|end| *end > 0) {
                cleaned.push_str(&after[..end]);
                remaining = &after[end + 1..];
            } else {
                remaining = &remaining[start..];
                break;
            }
        }
        cleaned.push_str(remaining);
        let mut words: Vec<_> = cleaned.split_whitespace().collect();
        const VERBS: [&str; 14] = [
            "use", "run", "execute", "create", "write", "read", "check", "verify", "update",
            "modify", "add", "remove", "delete", "install",
        ];
        if words.len() > 1 && VERBS.iter().any(|v| words[0].eq_ignore_ascii_case(v)) {
            words.remove(0);
            if words.len() > 1 && words[0].eq_ignore_ascii_case("the") {
                words.remove(0);
            }
        }
        let cleaned = words.join(" ");
        let mut cleaned = if let Some(first) = cleaned.chars().next() {
            format!("{}{}", first.to_uppercase(), &cleaned[first.len_utf8()..])
        } else {
            cleaned
        };
        // Match JavaScript's UTF-16 length and slice. A split surrogate stays unresolved.
        let units: Vec<_> = cleaned.encode_utf16().collect();
        if units.len() > 50 {
            cleaned = String::from_utf16(&units[..47]).ok()?;
            cleaned.push_str("...");
        }
        if cleaned.encode_utf16().count() > 3 {
            todos.push(
                serde_json::json!({"step": todos.len() + 1, "text": cleaned, "completed": false}),
            );
        }
    }
    Some(todos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    #[derive(Default)]
    struct Sink {
        unusable: Vec<PartialReason>,
    }

    impl RecordSink for Sink {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::Unusable(reason) = record {
                self.unusable.push(reason);
            }
        }
        fn finish(&mut self, _: crate::analysis::interface::SessionSummary) {}
    }

    fn row(parent: Option<String>) -> ScopeRow {
        ScopeRow {
            parent,
            complete: true,
            calls: Vec::new(),
            plan_state: None,
            assistant_text: None,
            assistant_bindings: Vec::new(),
            todo_list: None,
        }
    }

    fn restored(state: &PiScopeState) -> PiScopeState {
        postcard::from_bytes(&postcard::to_allocvec(state).unwrap()).unwrap()
    }

    fn result(parent: &str) -> Value {
        json!({"id":"result", "parentId":parent, "message":{"role":"toolResult", "toolName":"question", "toolCallId":"call-id", "details":{"question":"Scope?", "options":["API"], "answer":"API", "wasCustom":false}}})
    }

    #[test]
    fn restored_scope_cycles_fail_closed_without_partial_call_joins() {
        for two_cycle in [false, true] {
            let mut state = PiScopeState::default();
            let mut call = row(Some(if two_cycle { "b" } else { "a" }.into()));
            call.calls.push((
                "call-id".into(),
                "question".into(),
                json!({"question":"Scope?", "options":[{"label":"API"}]}).to_string(),
                0,
            ));
            state.rows.insert("a".into(), call);
            if two_cycle {
                state.rows.insert("b".into(), row(Some("a".into())));
            }
            let mut state = restored(&state);
            let mut sink = Sink::default();
            let parts = state.observe(&result("a"), &mut sink, &|| false).unwrap();
            assert!(parts.is_empty());
            assert!(state.incomplete());
            assert_eq!(sink.unusable, [PartialReason::AttributionIncomplete]);
        }
    }

    #[test]
    fn restored_scope_ancestry_has_a_fixed_bound_even_if_snapshot_rows_exceed_it() {
        let mut state = PiScopeState::default();
        for i in 0..=MAX_ROWS {
            state.rows.insert(
                format!("row-{i}"),
                row(i.checked_sub(1).map(|n| format!("row-{n}"))),
            );
        }
        let mut state = restored(&state);
        let valid = result(&format!("row-{}", MAX_ROWS - 1));
        assert_eq!(
            state.ancestors(&valid, &|| false).unwrap().unwrap().len(),
            MAX_ROWS
        );
        let steps = Cell::new(0);
        let cancel = || {
            steps.set(steps.get() + 1);
            false
        };
        let mut sink = Sink::default();
        assert!(
            state
                .observe(&result(&format!("row-{MAX_ROWS}")), &mut sink, &cancel)
                .unwrap()
                .is_empty()
        );
        assert_eq!(steps.get(), MAX_ROWS + 1);
        assert!(state.incomplete());
        assert_eq!(sink.unusable, [PartialReason::AttributionIncomplete]);
    }

    #[test]
    fn restored_scope_ancestry_checks_cancellation_during_traversal() {
        let mut state = PiScopeState::default();
        for i in 0usize..1000 {
            state.rows.insert(
                format!("row-{i}"),
                row(i.checked_sub(1).map(|n| format!("row-{n}"))),
            );
        }
        let mut state = restored(&state);
        let steps = Cell::new(0);
        let cancel = || {
            steps.set(steps.get() + 1);
            steps.get() == 4
        };
        let mut sink = Sink::default();
        let error = state
            .observe(&result("row-999"), &mut sink, &cancel)
            .unwrap_err();
        assert!(error.to_string().contains("ancestry read was cancelled"));
        assert_eq!(steps.get(), 4);
        assert!(!state.rows.contains_key("result"));
        assert!(sink.unusable.is_empty());
    }

    #[test]
    fn reader_propagates_cancellation_from_restored_scope_ancestry() {
        use super::super::{PiAdmission, PiDialect, PiSessionReader, PiStreamState};
        use std::io::{BufReader, Cursor};

        let mut scope = PiScopeState::default();
        for i in 0usize..1000 {
            scope.rows.insert(
                format!("row-{i}"),
                row(i.checked_sub(1).map(|n| format!("row-{n}"))),
            );
        }
        let state = PiStreamState {
            scope: restored(&scope),
            admission: PiAdmission::Accepted,
            ..Default::default()
        };
        let mut value = result("row-999");
        value["type"] = json!("message");
        value["timestamp"] = json!("2026-01-01T00:00:01Z");
        value["message"]["content"] = json!([{ "type":"text", "text":"User selected: 1. API" }]);
        let content = format!("{value}\n");
        let steps = Cell::new(0);
        let cancel = || {
            steps.set(steps.get() + 1);
            steps.get() == 100
        };
        let error = PiSessionReader
            .visit_reader_dialect(
                BufReader::new(Cursor::new(content)),
                &cancel,
                &mut Sink::default(),
                state,
                PiDialect::PI,
            )
            .unwrap_err();
        assert!(error.to_string().contains("ancestry read was cancelled"));
        assert_eq!(steps.get(), 100);
    }
}
