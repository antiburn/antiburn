//! Persisted Codex scope evidence. Tool completion does not prove human approval.

use std::collections::{BTreeMap, BTreeSet};

use crate::checks::ignored_instructions::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

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

const PRODUCER: &str = "openai/codex@e7637306bc9246a3e42e407cb94f96b7ed345e3e";
const MAX_CALLS: usize = 4096;
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct CodexScopeState {
    turn_id: Option<String>,
    calls: BTreeMap<String, Option<RecordedCall>>,
    bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedCall {
    input: String,
    turn_id: Option<String>,
    native_record_id: Option<String>,
}

impl CodexScopeState {
    pub(super) fn invalidate(&mut self) {
        for call in self.calls.values_mut() {
            *call = None;
        }
        self.bytes = 0;
    }

    pub(super) fn observe_context(&mut self, record: &Value) {
        if record["type"] == "turn_context" {
            self.turn_id = record["payload"]["turn_id"]
                .as_str()
                .filter(|id| valid_id(id))
                .map(str::to_owned);
        }
        if record["type"] == "event_msg" && record["payload"]["type"] == "thread_rolled_back" {
            self.invalidate();
        }
    }

    pub(super) fn capture(
        &mut self,
        record: &Value,
        parts: &mut Vec<ContentPart>,
        sink: &mut dyn RecordSink,
    ) {
        self.observe_context(record);
        let payload = &record["payload"];
        match (record["type"].as_str(), payload["type"].as_str()) {
            (Some("response_item"), Some("function_call")) => {
                self.call(record, parts, sink);
            }
            (Some("response_item"), Some("function_call_output")) => {
                self.output(record, parts, sink);
            }
            (Some("retained_context"), Some("verified_answer")) => {
                self.retained_answer(record, parts, sink);
            }
            (Some("response_item"), Some("message")) if payload["role"] == "assistant" => {
                for part in parts
                    .iter_mut()
                    .filter(|p| p.kind == ContentKind::AssistantText)
                {
                    if part.truncated {
                        continue;
                    }
                    let plans = proposed_plans(record, &part.text);
                    if !plans.is_empty() {
                        attach(part, Vec::new(), plans, sink);
                    }
                }
            }
            (Some("event_msg"), Some("item_completed")) if payload["item"]["type"] == "Plan" => {
                let mut reference = plan(
                    source(record, JevScopeEvidenceRole::Assistant, 0),
                    payload["item"]["text"].as_str().expect("validated plan"),
                );
                reference.plan_id = payload["item"]["id"].as_str().map(str::to_owned);
                push(
                    parts,
                    payload.to_string(),
                    Vec::new(),
                    vec![reference],
                    sink,
                );
            }
            (Some("compacted"), _) => {
                // Checkpoints retain bounded excerpts, not the complete authorization history.
                if let Some(checkpoint) = payload.get("retained_context") {
                    let mut part =
                        ContentPart::new(ContentKind::ToolResult, checkpoint.to_string());
                    let answers = checkpoint_answers(record, checkpoint);
                    attach(&mut part, answers, Vec::new(), sink);
                    if part.truncated {
                        sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
                    } else {
                        parts.push(part);
                    }
                    if checkpoint_incomplete(checkpoint) {
                        sink.record(NormalizedRecord::Unusable(
                            PartialReason::AttributionIncomplete,
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    fn call(&mut self, record: &Value, parts: &mut [ContentPart], sink: &mut dyn RecordSink) {
        let payload = &record["payload"];
        let Some(id) = payload["call_id"].as_str().filter(|id| valid_id(id)) else {
            return;
        };
        if let Some(previous) = self.calls.get_mut(id) {
            if let Some(previous) = previous.take() {
                self.bytes -= previous.input.len();
            }
            return;
        }
        if self.calls.len() >= MAX_CALLS {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
            return;
        }
        self.calls.insert(id.into(), None);
        if payload.get("namespace").is_some_and(|n| !n.is_null()) {
            return;
        }
        let Some(input) = payload["arguments"].as_str() else {
            return;
        };
        let Ok(arguments) = serde_json::from_str::<Value>(input) else {
            return;
        };
        if payload["name"] == "update_plan" {
            if valid_plan_update(&arguments) {
                let reference = plan(source(record, JevScopeEvidenceRole::Assistant, 0), input);
                if let Some(part) = parts.first_mut() {
                    attach(part, Vec::new(), vec![reference], sink);
                }
            }
            return;
        }
        if payload["name"] != "request_user_input" {
            return;
        }
        let Some(answers) = questions(record, &arguments) else {
            return;
        };
        if self.bytes.saturating_add(input.len()) > MAX_BYTES {
            sink.record(NormalizedRecord::Unusable(PartialReason::Oversized));
            return;
        }
        self.bytes += input.len();
        self.calls.insert(
            id.into(),
            Some(RecordedCall {
                input: input.into(),
                native_record_id: payload["id"].as_str().map(str::to_owned),
                turn_id: payload
                    .pointer("/internal_chat_message_metadata_passthrough/turn_id")
                    .map(|value| value.as_str().filter(|id| valid_id(id)).map(str::to_owned))
                    .unwrap_or_else(|| self.turn_id.clone()),
            }),
        );
        if let Some(part) = parts.first_mut() {
            attach(part, answers, Vec::new(), sink);
        }
    }

    fn output(&mut self, record: &Value, parts: &mut [ContentPart], sink: &mut dyn RecordSink) {
        let payload = &record["payload"];
        let Some(id) = payload["call_id"].as_str() else {
            return;
        };
        let Some(Some(call)) = self.calls.get(id) else {
            return;
        };
        if payload.get("namespace").is_some_and(|v| !v.is_null())
            || payload
                .get("name")
                .is_some_and(|v| !v.is_null() && v != "request_user_input")
            || payload
                .pointer("/internal_chat_message_metadata_passthrough/turn_id")
                .is_some_and(|value| value.as_str() != call.turn_id.as_deref())
        {
            return;
        }
        let arguments: Value = serde_json::from_str(&call.input).expect("validated arguments");
        let mut answers = questions(record, &arguments).expect("validated questions");
        let text = payload["output"].as_str();
        let result = text.and_then(|text| serde_json::from_str::<Value>(text).ok());
        let result = result
            .as_ref()
            .and_then(|result| result["answers"].as_object());
        let valid = result.is_some_and(|result| {
            result.keys().all(|id| {
                answers
                    .iter()
                    .any(|answer| answer.source.question_id.as_ref() == Some(id))
            }) && result.values().all(|answer| {
                answer["answers"]
                    .as_array()
                    .is_some_and(|values| values.iter().all(Value::is_string))
            })
        });
        let cancelled =
            text == Some("request_user_input was cancelled before receiving a response");
        for answer in &mut answers {
            answer.source.bindings.push(JevNativeFieldRange {
                native_record_id: call.native_record_id.clone(),
                field: JevInputField::UserAnswer,
                container: JevNativeFieldContainer::Record,
                pointer: "/payload/arguments".into(),
                start: 0,
                end: call.input.len(),
            });
            answer.source.role = JevScopeEvidenceRole::Tool;
            answer.status = if cancelled {
                JevUserAnswerStatus::Cancelled
            } else {
                JevUserAnswerStatus::Unknown
            };
            if !valid {
                continue;
            }
            let Some(values) = result.and_then(|r| {
                r.get(answer.source.question_id.as_deref().expect("question id"))?["answers"]
                    .as_array()
            }) else {
                continue;
            };
            if values.is_empty() {
                continue;
            }
            answer.status = JevUserAnswerStatus::Submitted;
            answer.selections = values
                .iter()
                .map(|value| {
                    let text = value.as_str().expect("validated answer");
                    let matching = answer
                        .options
                        .iter()
                        .enumerate()
                        .filter(|(_, option)| option.label == text)
                        .collect::<Vec<_>>();
                    let index = (matching.len() == 1).then(|| matching[0].0 as u32);
                    JevAnswerSelection {
                        option_id: None,
                        option_index: index,
                        value: Some(text.into()),
                        label: index.map(|_| text.into()),
                        custom: Some(matching.is_empty()),
                    }
                })
                .collect();
        }
        if let Some(part) = parts.first_mut() {
            attach(part, answers, Vec::new(), sink);
        }
    }

    fn retained_answer(
        &mut self,
        record: &Value,
        parts: &mut Vec<ContentPart>,
        sink: &mut dyn RecordSink,
    ) {
        let payload = &record["payload"];
        let rows = payload["questions"]
            .as_array()
            .expect("validated questions");
        let mut answers = Vec::new();
        if rows.is_empty() {
            sink.record(NormalizedRecord::Unusable(
                PartialReason::AttributionIncomplete,
            ));
        } else {
            answers = rows
                .iter()
                .enumerate()
                .map(|(index, row)| JevUserAnswer {
                    source: source(record, JevScopeEvidenceRole::Tool, index as u32),
                    prompt: row["question"].as_str().expect("question").into(),
                    header: None,
                    context: None,
                    comment: None,
                    options: Vec::new(),
                    multi_select: None,
                    selections: Vec::new(),
                    free_text: Some(row["answer"].as_str().expect("answer").into()),
                    status: JevUserAnswerStatus::Submitted,
                    origin: JevUserAnswerOrigin::UnknownOrigin,
                })
                .collect();
        }
        push(parts, payload.to_string(), answers, Vec::new(), sink);
    }
}

pub(super) fn is_retained_answer(payload: &Map<String, Value>) -> bool {
    ["turn_id", "call_id"].iter().all(|key| {
        payload
            .get(*key)
            .and_then(Value::as_str)
            .is_some_and(valid_id)
    }) && payload
        .get("acceptance_order")
        .is_none_or(|v| v.is_null() || v.as_u64().is_some())
        && payload
            .get("questions")
            .and_then(Value::as_array)
            .is_some_and(|rows| {
                rows.iter()
                    .all(|row| row["question"].is_string() && row["answer"].is_string())
            })
}

pub(super) fn is_completed_plan(payload: &Map<String, Value>) -> bool {
    payload.get("item").is_some_and(|item| {
        item["type"] == "Plan" && item["id"].is_string() && item["text"].is_string()
    })
}

fn questions(record: &Value, input: &Value) -> Option<Vec<JevUserAnswer>> {
    let rows = input["questions"].as_array()?;
    if rows.is_empty() {
        return None;
    }
    let mut ids = BTreeSet::new();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let id = row["id"].as_str().filter(|id| !id.is_empty())?;
            if !ids.insert(id) || !row["header"].is_string() {
                return None;
            }
            let options = row["options"].as_array()?;
            if options.is_empty() {
                return None;
            }
            let options = options
                .iter()
                .map(|option| {
                    Some(JevQuestionOption {
                        id: None,
                        value: None,
                        label: option["label"].as_str()?.into(),
                        description: Some(option["description"].as_str()?.into()),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let mut source = source(record, JevScopeEvidenceRole::Assistant, index as u32);
            source.question_id = Some(id.into());
            Some(JevUserAnswer {
                source,
                prompt: row["question"].as_str()?.into(),
                header: row["header"].as_str().map(str::to_owned),
                context: None,
                comment: None,
                options,
                multi_select: None,
                selections: Vec::new(),
                free_text: None,
                status: JevUserAnswerStatus::Pending,
                origin: JevUserAnswerOrigin::UnknownOrigin,
            })
        })
        .collect()
}

fn source(record: &Value, role: JevScopeEvidenceRole, order: u32) -> JevScopeEvidenceSource {
    let mut source = JevScopeEvidenceSource {
        source_format: SourceFormat::CodexRolloutJsonl,
        role,
        native_record_id: record["payload"]["id"].as_str().map(str::to_owned),
        call_id: record["payload"]["call_id"].as_str().map(str::to_owned),
        question_id: None,
        order,
        acceptance_order: record["payload"]["acceptance_order"].as_u64(),
        provenance: if record["payload"]["name"] == "update_plan"
            || role == JevScopeEvidenceRole::Assistant
                && record["payload"]["type"] != "function_call"
        {
            JevScopeEvidenceProvenance::RecordedAssistant
        } else {
            JevScopeEvidenceProvenance::RecognizedQuestionWorkflow
        },
        producer_revision: PRODUCER.into(),
        normalization_revision: 1,
        bindings: Vec::new(),
        truncated: false,
    };
    let field = if record["payload"]["name"] == "update_plan"
        || record["payload"]["item"]["type"] == "Plan"
    {
        JevInputField::PlanReference
    } else {
        JevInputField::UserAnswer
    };
    for pointer in [
        "/payload/arguments",
        "/payload/output",
        "/payload/item/text",
    ] {
        if let Some(text) = record.pointer(pointer).and_then(Value::as_str) {
            source.bindings.push(JevNativeFieldRange {
                native_record_id: source.native_record_id.clone(),
                field,
                container: JevNativeFieldContainer::Record,
                pointer: pointer.into(),
                start: 0,
                end: text.len(),
            });
        }
    }
    if record["type"] == "retained_context" {
        for key in ["question", "answer"] {
            if let Some(text) = record["payload"]["questions"][order as usize][key].as_str() {
                source.bindings.push(JevNativeFieldRange {
                    native_record_id: source.native_record_id.clone(),
                    field: JevInputField::UserAnswer,
                    container: JevNativeFieldContainer::Record,
                    pointer: format!("/payload/questions/{order}/{key}"),
                    start: 0,
                    end: text.len(),
                });
            }
        }
    }
    source
}

fn plan(mut source: JevScopeEvidenceSource, text: &str) -> JevPlanReference {
    source.provenance = JevScopeEvidenceProvenance::RecordedAssistant;
    let digest = sha256_hex(text.as_bytes());
    JevPlanReference {
        source,
        plan_id: None,
        path: None,
        revision: Some(digest.clone()),
        content_digest: Some(digest),
        approved_revision: None,
        approved_content_digest: None,
        text: Some(text.into()),
        feedback: None,
        status: JevPlanStatus::Proposed,
        origin: JevUserAnswerOrigin::UnknownOrigin,
        content_status: JevPlanContentStatus::Recorded,
    }
}

fn valid_plan_update(input: &Value) -> bool {
    input
        .get("explanation")
        .is_none_or(|v| v.is_null() || v.is_string())
        && input["plan"].as_array().is_some_and(|steps| {
            steps.iter().all(|step| {
                step["step"].is_string()
                    && matches!(
                        step["status"].as_str(),
                        Some("pending" | "in_progress" | "completed")
                    )
            })
        })
}

fn attach(
    part: &mut ContentPart,
    answers: Vec<JevUserAnswer>,
    plans: Vec<JevPlanReference>,
    sink: &mut dyn RecordSink,
) {
    match part.clone().with_scope_evidence(answers, plans) {
        Ok(captured) if !captured.truncated => *part = captured,
        _ => sink.record(NormalizedRecord::Unusable(PartialReason::Oversized)),
    }
}

fn push(
    parts: &mut Vec<ContentPart>,
    text: String,
    answers: Vec<JevUserAnswer>,
    plans: Vec<JevPlanReference>,
    sink: &mut dyn RecordSink,
) {
    let mut part = ContentPart::new(ContentKind::ToolResult, text);
    attach(&mut part, answers, plans, sink);
    if !part.truncated {
        parts.push(part)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 1024
}

fn proposed_plans(record: &Value, text: &str) -> Vec<JevPlanReference> {
    let mut start = None;
    let mut offset = 0;
    let mut ranges = Vec::new();
    for line in text.split_inclusive('\n') {
        match line.trim() {
            "<proposed_plan>" if start.is_none() => start = Some(offset + line.len()),
            "</proposed_plan>" if start.is_some() => {
                ranges.push((start.take().expect("open plan"), offset, false));
            }
            _ => {}
        }
        offset += line.len();
    }
    if let Some(start) = start {
        ranges.push((start, text.len(), true));
    }
    let block_index = record["payload"]["content"].as_array().and_then(|blocks| {
        blocks
            .iter()
            .position(|block| block["text"].as_str() == Some(text))
    });
    ranges
        .into_iter()
        .enumerate()
        .map(|(order, (start, end, truncated))| {
            let mut source = source(record, JevScopeEvidenceRole::Assistant, order as u32);
            source.truncated = truncated;
            if let Some(index) = block_index {
                source.bindings.push(JevNativeFieldRange {
                    native_record_id: source.native_record_id.clone(),
                    field: JevInputField::PlanReference,
                    container: JevNativeFieldContainer::Record,
                    pointer: format!("/payload/content/{index}/text"),
                    start,
                    end,
                });
            }
            plan(source, &text[start..end])
        })
        .collect()
}

fn checkpoint_answers(record: &Value, checkpoint: &Value) -> Vec<JevUserAnswer> {
    let Some(rows) = checkpoint["verified_answers"].as_array() else {
        return Vec::new();
    };
    rows.iter().enumerate().flat_map(|(index, row)| {
        let Some(questions) = row["questions"].as_array() else { return Vec::new() };
        if !row["turn_id"].as_str().is_some_and(valid_id) || !row["call_id"].as_str().is_some_and(valid_id)
            || row.get("inherited").is_some_and(|v| v != false)
            || questions.is_empty() || questions.iter().any(|q| !q["question"].is_string() || !q["answer"].is_string()) {
            return Vec::new();
        }
        questions.iter().enumerate().filter_map(|(order, question)| {
            let prompt = question["question"].as_str()?;
            let answer = question["answer"].as_str()?;
            let mut source = source(record, JevScopeEvidenceRole::Tool, order as u32);
            source.call_id = row["call_id"].as_str().map(str::to_owned);
            source.acceptance_order = row["order"].as_u64();
            source.truncated = checkpoint["incomplete"] != false;
            for (key, text) in [("question", prompt), ("answer", answer)] {
                source.bindings.push(JevNativeFieldRange { native_record_id: source.native_record_id.clone(), field: JevInputField::UserAnswer,
                    container: JevNativeFieldContainer::Record,
                    pointer: format!("/payload/retained_context/verified_answers/{index}/questions/{order}/{key}"), start: 0, end: text.len() });
            }
            Some(JevUserAnswer { source, prompt: prompt.into(), header: None, context: None, comment: None, options: Vec::new(), multi_select: None,
                selections: Vec::new(), free_text: Some(answer.into()), status: JevUserAnswerStatus::Submitted,
                origin: JevUserAnswerOrigin::UnknownOrigin })
        }).collect::<Vec<_>>()
    }).collect()
}

fn checkpoint_incomplete(checkpoint: &Value) -> bool {
    checkpoint["incomplete"] != false
        || checkpoint["user_messages_incomplete"] != false
        || !checkpoint["verified_answers"]
            .as_array()
            .is_some_and(|rows| {
                rows.iter().all(|row| {
                    row["turn_id"].as_str().is_some_and(valid_id)
                        && row["call_id"].as_str().is_some_and(valid_id)
                        && row["questions"].as_array().is_some_and(|questions| {
                            !questions.is_empty()
                                && questions
                                    .iter()
                                    .all(|q| q["question"].is_string() && q["answer"].is_string())
                        })
                })
            })
        || !checkpoint["user_messages"]
            .as_array()
            .is_some_and(|rows| rows.iter().all(|row| row["complete"] == true))
}
