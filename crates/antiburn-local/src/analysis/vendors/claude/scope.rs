//! Scoped Claude transcript evidence. Recorded tool completion does not prove user approval.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

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

const QUESTIONS: &str = concat!(
    "futpib/claudex@0ad5073179efbfcc9dd9d6a9c19cca4575431653;",
    "TrafficGuard/typedai@34139aec65bb70f7062cf7f92667c11ffde4fcb1"
);
const PLANS: &str = concat!(
    "vct-core@2.7.1:fc41d67b80db72fc9e13cd6a71e76846cc20a399fb8c9d3592849a42f58c2f82;",
    "folke/zaly@5a113518e6b0790fa0a63d058c3b5e76f8858e55"
);
const MAX_CALLS: usize = 4096;
const MAX_BYTES: usize = 1024 * 1024;

type ScopeRecords = (Vec<JevUserAnswer>, Vec<JevPlanReference>);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct ClaudeScopeState {
    calls: BTreeMap<String, Option<RecordedCall>>,
    bytes: usize,
    ancestry: super::ancestry::ClaudeAncestry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecordedCall {
    name: String,
    input: String,
    native_record_id: Option<String>,
    order: u32,
    native_results: bool,
}

impl ClaudeScopeState {
    pub(super) fn observe_record(&mut self, record: &Value) {
        self.ancestry.observe(record);
    }

    pub(super) fn invalidate(&mut self) {
        for call in self.calls.values_mut() {
            *call = None;
        }
        self.bytes = 0;
    }

    pub(super) fn capture(
        &mut self,
        record: &Value,
        branch: Option<&str>,
        parts: &mut Vec<ContentPart>,
        sink: &mut dyn RecordSink,
    ) {
        if record
            .pointer("/message/role")
            .is_some_and(|role| role != &record["type"])
        {
            self.invalidate();
            return;
        }
        if record["type"] == "user"
            && let Some(text) = record["planContent"].as_str()
        {
            let mut source = source(record, "", 0, false, false);
            source.call_id = None;
            source.role = JevScopeEvidenceRole::User;
            source.provenance = JevScopeEvidenceProvenance::Unknown;
            source.producer_revision = "anthropics/claude-code#24302:2026-03-19T14:31:09Z".into();
            let mut plan = plan_reference(source, None, Some(text));
            plan.origin = JevUserAnswerOrigin::Synthetic;
            bind_text(&mut plan, "/planContent");
            push(parts, sink, "ExitPlanMode", "", Vec::new(), vec![plan]);
        }
        let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        let result_count = blocks.iter().filter(|b| b["type"] == "tool_result").count();
        for (index, block) in blocks.iter().enumerate() {
            let order = index as u32;
            match (record["type"].as_str(), block["type"].as_str()) {
                (Some("assistant"), Some("tool_use")) => {
                    match self.capture_call(record, branch, block, order) {
                        Ok(Some((answers, plans))) => push(
                            parts,
                            sink,
                            block["name"].as_str().expect("validated name"),
                            block["id"].as_str().expect("validated id"),
                            answers,
                            plans,
                        ),
                        Ok(None) => {}
                        Err(reason) => sink.record(NormalizedRecord::Unusable(reason)),
                    }
                }
                (Some("user"), Some("tool_result")) => {
                    let Some(id) = block["tool_use_id"].as_str() else {
                        continue;
                    };
                    let Some(key) = call_key(record, branch, id) else {
                        continue;
                    };
                    let Some(Some(call)) = self.calls.get(&key) else {
                        continue;
                    };
                    if !self
                        .ancestry
                        .descends_from(record, call.native_record_id.as_deref())
                        || record
                            .get("sourceToolUseID")
                            .is_some_and(|value| value.as_str() != Some(id))
                        || record
                            .get("sourceToolAssistantUUID")
                            .is_some_and(|value| value.as_str() != call.native_record_id.as_deref())
                    {
                        continue;
                    }
                    let Some(Some(call)) = self.calls.get_mut(&key).map(Option::take) else {
                        continue;
                    };
                    self.bytes -= call.input.len();
                    let Ok(input) = serde_json::from_str::<Value>(&call.input) else {
                        continue;
                    };
                    let structured = (result_count == 1
                        && record
                            .get("sourceToolUseID")
                            .is_none_or(|v| v.as_str() == Some(id)))
                    .then(|| record.get("toolUseResult"))
                    .flatten();
                    let source = source(record, id, order, false, call.name == "AskUserQuestion");
                    let mut input_source = source.clone();
                    input_source.role = JevScopeEvidenceRole::Assistant;
                    input_source.native_record_id = call.native_record_id;
                    input_source.order = call.order;
                    if call.native_results {
                        super::native::capture_result(
                            record, block, &input, &call.name, structured, order, parts,
                        );
                    }
                    let (answers, plans) = match call.name.as_str() {
                        "AskUserQuestion" => (
                            result_answers(
                                source,
                                &input_source,
                                &input,
                                structured,
                                record,
                                block,
                            ),
                            Vec::new(),
                        ),
                        "ExitPlanMode" => (
                            Vec::new(),
                            vec![plan_result(source, &input, structured, record, block)],
                        ),
                        "Read" | "Write" => (
                            Vec::new(),
                            file_plan(source, &call.name, &input, structured, record, block),
                        ),
                        _ => continue,
                    };
                    push(parts, sink, &call.name, id, answers, plans);
                }
                _ => {}
            }
        }
    }

    fn capture_call(
        &mut self,
        record: &Value,
        branch: Option<&str>,
        block: &Value,
        order: u32,
    ) -> Result<Option<ScopeRecords>, PartialReason> {
        let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) else {
            return Ok(None);
        };
        let Some(key) = call_key(record, branch, id) else {
            return Ok(None);
        };
        if let Some(previous) = self.calls.get_mut(&key) {
            if let Some(previous) = previous.take() {
                self.bytes -= previous.input.len();
            }
            return Ok(None);
        }
        if self.calls.len() >= MAX_CALLS {
            return Err(PartialReason::AttributionIncomplete);
        }
        self.calls.insert(key.clone(), None);
        if !matches!(
            name,
            "AskUserQuestion" | "ExitPlanMode" | "Read" | "Write" | "Bash" | "Skill"
        ) || !block["input"].is_object()
        {
            return Ok(None);
        }
        let input = block["input"].to_string();
        if self.bytes + input.len() > MAX_BYTES {
            return Err(PartialReason::Oversized);
        }
        self.bytes += input.len();
        self.calls.insert(
            key,
            Some(RecordedCall {
                name: name.into(),
                input,
                native_record_id: record["uuid"].as_str().map(str::to_owned),
                order,
                native_results: record["version"] == "2.1.278",
            }),
        );
        let source = source(record, id, order, true, name == "AskUserQuestion");
        match name {
            "AskUserQuestion" => {
                let answers = questions(&block["input"], &source);
                Ok(Some((answers, Vec::new())))
            }
            "ExitPlanMode" => {
                let mut plan = plan_reference(
                    source,
                    block["input"]["planFilePath"].as_str(),
                    block["input"]["plan"].as_str(),
                );
                plan.status = JevPlanStatus::Proposed;
                bind_text(&mut plan, &format!("/message/content/{order}/input/plan"));
                Ok(Some((Vec::new(), vec![plan])))
            }
            "Write"
                if block["input"]["file_path"]
                    .as_str()
                    .is_some_and(is_plan_path) =>
            {
                let mut plan = plan_reference(
                    source,
                    block["input"]["file_path"].as_str(),
                    block["input"]["content"].as_str(),
                );
                plan.status = JevPlanStatus::Proposed;
                bind_text(
                    &mut plan,
                    &format!("/message/content/{order}/input/content"),
                );
                Ok(Some((Vec::new(), vec![plan])))
            }
            _ => Ok(None),
        }
    }
}

fn call_key(record: &Value, branch: Option<&str>, id: &str) -> Option<String> {
    let session = record
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let agent = record.get("agentId").and_then(Value::as_str).unwrap_or("");
    let branch = branch.unwrap_or("");
    if id.is_empty() || [id, session, agent, branch].iter().any(|s| s.len() > 512) {
        return None;
    }
    Some(
        serde_json::to_string(&(session, agent, branch, record["isSidechain"] == true, id))
            .expect("string tuple serializes"),
    )
}

fn source(
    record: &Value,
    id: &str,
    order: u32,
    assistant: bool,
    question: bool,
) -> JevScopeEvidenceSource {
    JevScopeEvidenceSource {
        source_format: SourceFormat::ClaudeJsonl,
        role: if assistant {
            JevScopeEvidenceRole::Assistant
        } else {
            JevScopeEvidenceRole::Tool
        },
        native_record_id: record["uuid"].as_str().map(str::to_owned),
        call_id: Some(id.into()),
        question_id: None,
        order,
        acceptance_order: None,
        provenance: if question {
            JevScopeEvidenceProvenance::RecognizedQuestionWorkflow
        } else {
            JevScopeEvidenceProvenance::RecognizedPlanWorkflow
        },
        producer_revision: if question { QUESTIONS } else { PLANS }.into(),
        normalization_revision: 1,
        bindings: Vec::new(),
        truncated: false,
    }
}

fn questions(input: &Value, source: &JevScopeEvidenceSource) -> Vec<JevUserAnswer> {
    let Some(raw) = input["questions"].as_array() else {
        return Vec::new();
    };
    raw.iter()
        .enumerate()
        .filter_map(|(index, q)| {
            let prompt = q["question"].as_str()?;
            let options: Option<Vec<_>> = q["options"]
                .as_array()?
                .iter()
                .map(|o| {
                    Some(JevQuestionOption {
                        id: None,
                        value: None,
                        label: o["label"].as_str()?.into(),
                        description: Some(o["description"].as_str()?.into()),
                    })
                })
                .collect();
            let options = options?;
            let multi_select = match q.get("multiSelect") {
                None => None,
                Some(v) => Some(v.as_bool()?),
            };
            let mut source = source.clone();
            source.question_id = Some(index.to_string());
            let pointer = if source.role == JevScopeEvidenceRole::Assistant {
                format!("/message/content/{}/input/questions/{index}", source.order)
            } else {
                format!("/toolUseResult/questions/{index}")
            };
            for key in ["question", "header"] {
                if let Some(text) = q[key].as_str() {
                    source.bindings.push(binding(
                        JevInputField::UserAnswer,
                        format!("{pointer}/{key}"),
                        text.len(),
                        source.native_record_id.clone(),
                    ));
                }
            }
            for (i, option) in q["options"].as_array()?.iter().enumerate() {
                for key in ["label", "description"] {
                    if let Some(text) = option[key].as_str() {
                        source.bindings.push(binding(
                            JevInputField::UserAnswer,
                            format!("{pointer}/options/{i}/{key}"),
                            text.len(),
                            source.native_record_id.clone(),
                        ));
                    }
                }
            }
            Some(JevUserAnswer {
                source,
                prompt: prompt.into(),
                header: q["header"].as_str().map(str::to_owned),
                context: None,
                comment: None,
                options,
                multi_select,
                selections: Vec::new(),
                free_text: None,
                status: JevUserAnswerStatus::Pending,
                origin: JevUserAnswerOrigin::UnknownOrigin,
            })
        })
        .collect()
}

fn intact(record: &Value, block: &Value) -> bool {
    block.get("is_error").is_none_or(|v| v == false)
        && [
            "interruptedByShutdown",
            "isMeta",
            "isCompactSummary",
            "isSynthetic",
        ]
        .iter()
        .all(|key| record.get(key).is_none_or(|v| v == false))
        && record.get("toolDenialKind").is_none()
        && ["interrupted", "truncated"].iter().all(|key| {
            record
                .get("toolUseResult")
                .and_then(|p| p.get(key))
                .is_none_or(|v| v == false)
        })
}

fn result_answers(
    source: JevScopeEvidenceSource,
    input_source: &JevScopeEvidenceSource,
    input: &Value,
    structured: Option<&Value>,
    record: &Value,
    block: &Value,
) -> Vec<JevUserAnswer> {
    let mut answers = if let Some(payload) = structured.filter(|p| p.get("questions").is_some()) {
        questions(payload, &source)
    } else {
        questions(input, input_source)
    };
    for answer in &mut answers {
        let bindings = std::mem::take(&mut answer.source.bindings);
        let question_id = answer.source.question_id.take();
        answer.source = source.clone();
        answer.source.question_id = question_id;
        answer.source.bindings = bindings;
        answer.status = JevUserAnswerStatus::Unknown;
    }
    let Some(payload) = structured else {
        return answers;
    };
    let Some(values) = payload["answers"].as_object() else {
        return answers;
    };
    let conflict = payload
        .get("questions")
        .is_some_and(|q| input.get("questions").is_some_and(|original| original != q));
    if conflict {
        let mut original = questions(input, input_source);
        for answer in &mut original {
            let bindings = std::mem::take(&mut answer.source.bindings);
            let question_id = answer.source.question_id.take();
            answer.source = source.clone();
            answer.source.question_id = question_id;
            answer.source.bindings = bindings;
            answer.status = JevUserAnswerStatus::Unknown;
        }
        answers.extend(original);
    }
    for (prompt, value) in values {
        let Some(value) = value.as_str() else {
            continue;
        };
        let position = answers.iter().position(|q| &q.prompt == prompt);
        let ambiguous = answers.iter().filter(|q| &q.prompt == prompt).count() > 1;
        let answer = if let Some(position) = position {
            &mut answers[position]
        } else {
            answers.push(JevUserAnswer {
                source: source.clone(),
                prompt: prompt.clone(),
                header: None,
                context: None,
                comment: None,
                options: Vec::new(),
                multi_select: None,
                selections: Vec::new(),
                free_text: None,
                status: JevUserAnswerStatus::Unknown,
                origin: JevUserAnswerOrigin::UnknownOrigin,
            });
            answers.last_mut().expect("answer was inserted")
        };
        answer.source.bindings.push(binding(
            JevInputField::UserAnswer,
            format!(
                "/toolUseResult/answers/{}",
                prompt.replace('~', "~0").replace('/', "~1")
            ),
            value.len(),
            source.native_record_id.clone(),
        ));
        // A string can combine selections and free text. Do not split delimiters.
        answer.free_text = Some(value.into());
        let indices: Vec<_> = answer
            .options
            .iter()
            .enumerate()
            .filter(|(_, o)| o.label == value)
            .map(|(i, _)| i)
            .collect();
        let option_index =
            (indices.len() == 1 && !conflict && !ambiguous).then(|| indices[0] as u32);
        answer.selections.push(JevAnswerSelection {
            option_id: None,
            option_index,
            value: Some(value.into()),
            label: option_index.map(|_| value.into()),
            custom: None,
        });
        answer.status = if intact(record, block) && !conflict && !ambiguous && !value.is_empty() {
            JevUserAnswerStatus::Submitted
        } else {
            JevUserAnswerStatus::Unknown
        };
    }
    answers
}

fn plan_reference(
    source: JevScopeEvidenceSource,
    path: Option<&str>,
    text: Option<&str>,
) -> JevPlanReference {
    let digest = text.map(|t| format!("{:x}", Sha256::digest(t.as_bytes())));
    JevPlanReference {
        source,
        plan_id: None,
        path: path.map(str::to_owned),
        revision: digest.clone(),
        content_digest: digest,
        approved_revision: None,
        approved_content_digest: None,
        text: text.map(str::to_owned),
        feedback: None,
        status: JevPlanStatus::Unknown,
        origin: JevUserAnswerOrigin::UnknownOrigin,
        content_status: if text.is_some() {
            JevPlanContentStatus::Recorded
        } else {
            JevPlanContentStatus::Unresolved
        },
    }
}

fn binding(
    field: JevInputField,
    pointer: String,
    end: usize,
    native_record_id: Option<String>,
) -> JevNativeFieldRange {
    JevNativeFieldRange {
        native_record_id,
        field,
        container: JevNativeFieldContainer::Record,
        pointer,
        start: 0,
        end,
    }
}

fn bind_text(plan: &mut JevPlanReference, pointer: &str) {
    if let Some(text) = &plan.text {
        plan.source.bindings.push(binding(
            JevInputField::PlanReference,
            pointer.into(),
            text.len(),
            plan.source.native_record_id.clone(),
        ));
    }
}

fn plan_result(
    source: JevScopeEvidenceSource,
    input: &Value,
    structured: Option<&Value>,
    record: &Value,
    block: &Value,
) -> JevPlanReference {
    let payload = structured.filter(|_| intact(record, block));
    let mut plan = plan_reference(
        source,
        payload
            .and_then(|p| p["filePath"].as_str())
            .or_else(|| input["planFilePath"].as_str()),
        payload.and_then(|p| p["plan"].as_str()),
    );
    bind_text(&mut plan, "/toolUseResult/plan");
    // Tool completion does not record the approval origin.
    plan.feedback = block["content"].as_str().map(str::to_owned);
    plan
}

fn file_plan(
    source: JevScopeEvidenceSource,
    name: &str,
    input: &Value,
    structured: Option<&Value>,
    record: &Value,
    block: &Value,
) -> Vec<JevPlanReference> {
    let Some(path) = input["file_path"].as_str().filter(|p| is_plan_path(p)) else {
        return Vec::new();
    };
    let payload = structured.filter(|_| intact(record, block));
    let text = match (name, payload) {
        ("Read", Some(p))
            if p["type"] == "text"
                && p["file"]["filePath"] == path
                && p["file"]["startLine"] == 1
                && p["file"]["numLines"].as_u64().is_some()
                && p["file"]["numLines"] == p["file"]["totalLines"] =>
        {
            p["file"]["content"].as_str()
        }
        ("Write", Some(p))
            if matches!(p["type"].as_str(), Some("create" | "update"))
                && p["filePath"] == path
                && p["content"] == input["content"] =>
        {
            p["content"].as_str()
        }
        _ => None,
    };
    let mut plan = plan_reference(source, Some(path), text);
    bind_text(
        &mut plan,
        if name == "Read" {
            "/toolUseResult/file/content"
        } else {
            "/toolUseResult/content"
        },
    );
    vec![plan]
}

fn is_plan_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let Some((_, file)) = normalized.split_once("/.claude/plans/") else {
        return false;
    };
    !file.contains('/') && file.ends_with(".md") && file != ".md"
}

fn push(
    parts: &mut Vec<ContentPart>,
    sink: &mut dyn RecordSink,
    name: &str,
    id: &str,
    answers: Vec<JevUserAnswer>,
    plans: Vec<JevPlanReference>,
) {
    if answers.is_empty() && plans.is_empty() {
        return;
    }
    let part = ContentPart::new(ContentKind::ToolResult, "")
        .with_tool_identity(Some(name.into()), Some(id.into()));
    match part.with_scope_evidence(answers, plans) {
        Ok(part) => parts.push(part),
        Err(_) => sink.record(NormalizedRecord::Unusable(PartialReason::Oversized)),
    }
}
