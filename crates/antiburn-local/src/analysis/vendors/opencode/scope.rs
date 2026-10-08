//! OpenCode scope records from producer 772392050500e0ddcd2ad2193411a22a3824372f.

use super::*;
use crate::analysis::SourceFormat;
use crate::analysis::jev_evidence::{
    JevAnswerSelection, JevOperationState, JevPlanContentStatus, JevPlanReference, JevPlanStatus,
    JevQuestionOption, JevScopeEvidenceProvenance, JevScopeEvidenceRole, JevScopeEvidenceSource,
    JevUserAnswer, JevUserAnswerOrigin, JevUserAnswerStatus,
};

const PRODUCER: &str = "anomalyco/opencode@772392050500e0ddcd2ad2193411a22a3824372f";

pub(super) fn is_build_user(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("user")
        && message.get("agent").and_then(Value::as_str) == Some("build")
        && message
            .pointer("/model/providerID")
            .and_then(Value::as_str)
            .is_some()
        && message
            .pointer("/model/modelID")
            .and_then(Value::as_str)
            .is_some()
}

fn source(part: &Map<String, Value>, order: u32, plan: bool) -> JevScopeEvidenceSource {
    JevScopeEvidenceSource {
        source_format: SourceFormat::OpenCodeJsonl,
        role: JevScopeEvidenceRole::Tool,
        native_record_id: part.get("id").and_then(Value::as_str).map(str::to_owned),
        call_id: part
            .get("callID")
            .and_then(Value::as_str)
            .map(str::to_owned),
        question_id: None,
        order,
        acceptance_order: None,
        provenance: if !local_execution(part) {
            JevScopeEvidenceProvenance::Unknown
        } else if plan {
            JevScopeEvidenceProvenance::RecognizedPlanWorkflow
        } else {
            JevScopeEvidenceProvenance::RecognizedQuestionWorkflow
        },
        producer_revision: PRODUCER.into(),
        normalization_revision: 2,
        bindings: Vec::new(),
        truncated: false,
    }
}

fn local_execution(part: &Map<String, Value>) -> bool {
    part.get("metadata").is_none_or(|metadata| {
        metadata.as_object().is_some_and(|metadata| {
            metadata
                .get("providerExecuted")
                .is_none_or(|value| value == &Value::Bool(false))
        })
    })
}

fn answer_status(state: &Value) -> JevUserAnswerStatus {
    match state.get("status").and_then(Value::as_str) {
        Some("pending" | "running") => JevUserAnswerStatus::Pending,
        Some("error")
            if state.get("error").and_then(Value::as_str)
                == Some("The user dismissed this question") =>
        {
            JevUserAnswerStatus::Cancelled
        }
        _ => JevUserAnswerStatus::Unknown,
    }
}

fn intact(state: &Value) -> bool {
    state
        .pointer("/metadata/truncated")
        .is_none_or(|v| v == &Value::Bool(false))
        && state
            .pointer("/metadata/interrupted")
            .is_none_or(|v| v == &Value::Bool(false))
        && state.pointer("/time/compacted").is_none()
}

fn completed_time(state: &Value) -> bool {
    matches!(
        (state.pointer("/time/start").and_then(Value::as_u64), state.pointer("/time/end").and_then(Value::as_u64)),
        (Some(start), Some(end)) if start <= end
    )
}

fn tool_content(part: &Map<String, Value>, state: &Value, name: &str) -> ContentPart {
    let mut captured = ContentPart::new(ContentKind::ToolResult, "").with_tool_identity(
        Some(name.into()),
        part.get("callID")
            .and_then(Value::as_str)
            .map(str::to_owned),
    );
    captured.metadata.state = match state.get("status").and_then(Value::as_str) {
        Some("pending") => JevOperationState::Pending,
        Some("running") => JevOperationState::Running,
        Some("completed") => JevOperationState::Completed,
        Some("error") => JevOperationState::Error,
        _ => JevOperationState::Unknown,
    };
    captured
}

pub(super) fn capture_tool(
    part: &Map<String, Value>,
    pending: &mut PendingMessage,
    sink: &mut dyn RecordSink,
) {
    if pending.event.role != Role::Assistant
        || pending.event.thread_id.is_none()
        || part
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || part
            .get("callID")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        || part
            .get("messageID")
            .and_then(Value::as_str)
            .is_some_and(|id| id != pending.id)
        || part
            .get("sessionID")
            .and_then(Value::as_str)
            .is_some_and(|id| Some(id) != pending.event.thread_id.as_deref())
    {
        return;
    }
    let Some(state) = part.get("state") else {
        return;
    };
    match part.get("tool").and_then(Value::as_str) {
        Some("question") => capture_questions(part, state, pending, sink),
        Some("plan_exit") => capture_plan_exit(part, state, pending, sink),
        _ => {}
    }
}

fn capture_questions(
    part: &Map<String, Value>,
    state: &Value,
    pending: &mut PendingMessage,
    sink: &mut dyn RecordSink,
) {
    let Some(questions) = state.pointer("/input/questions").and_then(Value::as_array) else {
        return;
    };
    let mut answers = Vec::new();
    for (index, question) in questions.iter().enumerate() {
        let Some(prompt) = question.get("question").and_then(Value::as_str) else {
            return;
        };
        if question.get("header").and_then(Value::as_str).is_none() {
            return;
        }
        let Some(options) = question.get("options").and_then(Value::as_array) else {
            return;
        };
        let options: Option<Vec<_>> = options
            .iter()
            .map(|option| {
                Some(JevQuestionOption {
                    id: None,
                    value: None,
                    label: option.get("label")?.as_str()?.into(),
                    description: Some(option.get("description")?.as_str()?.into()),
                })
            })
            .collect();
        let Some(options) = options else { return };
        if question.get("multiple").is_some_and(|v| !v.is_boolean()) {
            return;
        }
        answers.push(JevUserAnswer {
            source: source(part, index as u32, false),
            prompt: prompt.into(),
            header: question["header"].as_str().map(str::to_owned),
            context: None,
            comment: None,
            options,
            multi_select: Some(
                question
                    .get("multiple")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
            selections: Vec::new(),
            free_text: None,
            status: if local_execution(part) {
                answer_status(state)
            } else {
                JevUserAnswerStatus::Unknown
            },
            origin: JevUserAnswerOrigin::UnknownOrigin,
        });
    }
    let title = format!(
        "Asked {} question{}",
        answers.len(),
        if answers.len() > 1 { "s" } else { "" }
    );
    let submitted = if state.get("status").and_then(Value::as_str) == Some("completed")
        && intact(state)
        && completed_time(state)
        && state.get("title").and_then(Value::as_str) == Some(title.as_str())
        && state.get("metadata").and_then(Value::as_object).is_some()
    {
        recorded_answers(state, &answers)
    } else {
        None
    };
    if let Some(submitted) = submitted {
        for (answer, values) in answers.iter_mut().zip(submitted) {
            if values.is_empty() {
                continue;
            }
            if local_execution(part) {
                answer.status = JevUserAnswerStatus::Submitted;
                answer.origin = JevUserAnswerOrigin::User;
            }
            for value in values {
                let indices: Vec<_> = answer
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| o.label == value)
                    .map(|(i, _)| i)
                    .collect();
                let option_index = (indices.len() == 1).then(|| indices[0] as u32);
                answer.selections.push(JevAnswerSelection {
                    option_id: None,
                    option_index,
                    value: Some(value.clone()),
                    label: option_index.map(|_| value),
                    custom: if indices.is_empty() {
                        Some(true)
                    } else {
                        option_index.map(|_| false)
                    },
                });
            }
            if answer.selections.len() == 1 && answer.selections[0].custom == Some(true) {
                answer.free_text = answer.selections[0].value.clone();
            }
        }
    }
    let captured = tool_content(part, state, "question");
    push_scope(captured, answers, Vec::new(), pending, sink);
}

fn formatted(answers: &[JevUserAnswer], values: &[Vec<String>]) -> String {
    let pairs = answers
        .iter()
        .zip(values)
        .map(|(q, a)| {
            format!(
                "\"{}\"=\"{}\"",
                q.prompt,
                if a.is_empty() {
                    "Unanswered".into()
                } else {
                    a.join(", ")
                }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "User has answered your questions: {pairs}. You can now continue with the user's answers in mind."
    )
}

fn recorded_answers(state: &Value, questions: &[JevUserAnswer]) -> Option<Vec<Vec<String>>> {
    let output = state.get("output")?.as_str()?;
    if let Some(raw) = state.pointer("/metadata/answers") {
        let values: Vec<Vec<String>> = serde_json::from_value(raw.clone()).ok()?;
        if values.len() != questions.len() || formatted(questions, &values) != output {
            return None;
        }
        return Some(values);
    }
    // Text cannot distinguish delimiters inside custom answers or multiple prompts.
    if questions.len() != 1 {
        return None;
    }
    let q = &questions[0];
    let value = output
        .strip_prefix(&format!(
            "User has answered your questions: \"{}\"=\"",
            q.prompt
        ))?
        .strip_suffix("\". You can now continue with the user's answers in mind.")?;
    if value.contains(['"', ',', '\n']) || value == "Unanswered" {
        return None;
    }
    Some(vec![vec![value.into()]])
}

fn plan(
    source: JevScopeEvidenceSource,
    path: Option<String>,
    status: JevPlanStatus,
    origin: JevUserAnswerOrigin,
) -> JevPlanReference {
    JevPlanReference {
        source,
        plan_id: None,
        path,
        revision: None,
        content_digest: None,
        approved_revision: None,
        approved_content_digest: None,
        text: None,
        feedback: None,
        status,
        origin,
        content_status: JevPlanContentStatus::Unresolved,
    }
}

fn capture_plan_exit(
    part: &Map<String, Value>,
    state: &Value,
    pending: &mut PendingMessage,
    sink: &mut dyn RecordSink,
) {
    if !state
        .get("input")
        .and_then(Value::as_object)
        .is_some_and(Map::is_empty)
    {
        return;
    }
    let completed = local_execution(part)
        && state.get("status").and_then(Value::as_str) == Some("completed")
        && state.get("title").and_then(Value::as_str) == Some("Switching to build agent")
        && state.get("output").and_then(Value::as_str)
            == Some("User approved switching to build agent. Wait for further instructions.")
        && state
            .get("metadata")
            .and_then(Value::as_object)
            .is_some_and(|metadata| {
                metadata.len() == 1 && metadata.get("truncated") == Some(&Value::Bool(false))
            })
        && intact(state);
    let completed = completed && completed_time(state);
    let status = if completed {
        JevPlanStatus::Approved
    } else if !local_execution(part) {
        JevPlanStatus::Unknown
    } else {
        match answer_status(state) {
            JevUserAnswerStatus::Cancelled => JevPlanStatus::Cancelled,
            JevUserAnswerStatus::Pending => JevPlanStatus::Pending,
            _ => JevPlanStatus::Unknown,
        }
    };
    let reference = plan(
        source(part, 0, true),
        None,
        status,
        if completed {
            JevUserAnswerOrigin::User
        } else {
            JevUserAnswerOrigin::UnknownOrigin
        },
    );
    push_scope(
        tool_content(part, state, "plan_exit"),
        Vec::new(),
        vec![reference],
        pending,
        sink,
    );
}

pub(super) fn capture_synthetic_plan(
    part: &Map<String, Value>,
    pending: &mut PendingMessage,
    captured: ContentPart,
    sink: &mut dyn RecordSink,
) {
    if pending.build_user
        && !captured.truncated
        && part
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        && part.get("synthetic").and_then(Value::as_bool) == Some(true)
        && part
            .get("messageID")
            .and_then(Value::as_str)
            .is_none_or(|id| id == pending.id)
        && part
            .get("sessionID")
            .and_then(Value::as_str)
            .is_none_or(|id| Some(id) == pending.event.thread_id.as_deref())
        && let Some(path) = captured.text.strip_prefix("The plan at ").and_then(|t| {
            t.strip_suffix(" has been approved, you can now edit files. Execute the plan")
        })
        && !path.is_empty()
    {
        let mut binding = source(part, 0, true);
        binding.role = JevScopeEvidenceRole::User;
        binding.provenance = JevScopeEvidenceProvenance::Unknown;
        let reference = plan(
            binding,
            Some(path.into()),
            JevPlanStatus::Unknown,
            JevUserAnswerOrigin::Synthetic,
        );
        push_scope(captured, Vec::new(), vec![reference], pending, sink);
    } else {
        pending.content.push(captured);
    }
}

fn push_scope(
    captured: ContentPart,
    answers: Vec<JevUserAnswer>,
    plans: Vec<JevPlanReference>,
    pending: &mut PendingMessage,
    sink: &mut dyn RecordSink,
) {
    match captured.with_scope_evidence(answers, plans) {
        Ok(captured) => pending.content.push(captured),
        Err(_) => sink.record(NormalizedRecord::Unusable(PartialReason::Oversized)),
    }
}
