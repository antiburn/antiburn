use antiburn_checks::compiler::StatusResult;
use antiburn_local::analysis::{
    SourceFormat, ToolCategory, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus, UserMessage, UserMessageChange,
    UserMessageCompleteness, UserMessageOrigin, UserMessagePart, UserMessagePosition,
    UserMessageScope,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::store::ToolResultBatch;

pub(super) struct Definition {
    pub id: &'static str,
    pub path: &'static str,
    pub yaml: &'static str,
}

pub(super) const DEFINITION: Definition = Definition {
    id: "synthetic-retry-review",
    path: "crates/antiburn-checks/tests/fixtures/retry-review.yml",
    yaml: include_str!("../../../../../crates/antiburn-checks/tests/fixtures/retry-review.yml"),
};

pub(super) struct Case {
    pub id: &'static str,
    pub definition: &'static Definition,
    pub fixture: Fixture,
    pub expected: &'static [Expected],
}

#[derive(Clone, Copy)]
pub(super) struct Expected {
    pub trigger: u64,
    pub window: usize,
    pub label: &'static str,
    pub status_write: Option<StatusResult>,
}

#[derive(Clone, Copy)]
pub(super) struct Fixture {
    pub request: &'static str,
    pub follow_up: Option<&'static str>,
    pub results: &'static [ToolResult],
    pub complete_context: bool,
    pub context_available: bool,
}

pub(super) struct ToolResult {
    pub command: &'static str,
    pub content: &'static str,
    pub failed: bool,
}

const FAILURE: ToolResult = ToolResult {
    command: "cargo test parser",
    content: "test parser::tests::parse_empty FAILED: expected an empty list, got null; src/parser.rs:42",
    failed: true,
};
const RECOVERY: ToolResult = ToolResult {
    command: "python3 tools/fix_empty_parser.py && cargo test parser",
    content: "Updated src/parser.rs: parse_empty now returns [] instead of null. cargo test parser: 12 passed; 0 failed.",
    failed: false,
};
const POLLING: ToolResult = ToolResult {
    command: "cargo test parser",
    content: "fixture service not ready: HTTP 503; retry expected during service startup",
    failed: true,
};
const UNCERTAIN: ToolResult = ToolResult {
    command: "python3 tools/probe_parser_service.py",
    content: "Command failed. No diagnostic output is available. The cause and relationship to the parser test failure are unknown.",
    failed: true,
};
const STUCK: Expected = Expected {
    trigger: 3,
    window: 3,
    label: "stuck",
    status_write: Some(StatusResult::Failure),
};
const CLEAR: Expected = Expected {
    trigger: 3,
    window: 3,
    label: "clear",
    status_write: Some(StatusResult::Success),
};
const UNCERTAIN_EXPECTED: Expected = Expected {
    trigger: 3,
    window: 3,
    label: "uncertain",
    status_write: Some(StatusResult::Success),
};

pub(super) fn cases() -> Vec<Case> {
    let repeated = Fixture {
        request: "Make the parser tests pass. Identify and fix the cause of any test failure.",
        follow_up: None,
        results: &[FAILURE, FAILURE, FAILURE],
        complete_context: true,
        context_available: true,
    };
    vec![
        Case {
            id: "repeated-failure",
            definition: &DEFINITION,
            fixture: repeated,
            expected: &[STUCK],
        },
        Case {
            id: "rolling-recovery",
            definition: &DEFINITION,
            fixture: Fixture {
                follow_up: Some("Apply the minimal parser fix, then run cargo test parser again."),
                results: &[FAILURE, FAILURE, FAILURE, RECOVERY],
                ..repeated
            },
            expected: &[
                STUCK,
                Expected {
                    trigger: 4,
                    window: 4,
                    ..CLEAR
                },
            ],
        },
        Case {
            id: "requested-retry",
            definition: &DEFINITION,
            fixture: Fixture {
                request: "Poll cargo test parser three times while the external parser fixture service starts. Temporary fixture-service failures are expected; do not edit parser code.",
                results: &[POLLING, POLLING, POLLING],
                ..repeated
            },
            expected: &[CLEAR],
        },
        Case {
            id: "uncertain-after-failure",
            definition: &DEFINITION,
            fixture: Fixture {
                results: &[FAILURE, FAILURE, FAILURE, UNCERTAIN],
                ..repeated
            },
            expected: &[
                STUCK,
                Expected {
                    trigger: 4,
                    window: 4,
                    label: "uncertain",
                    status_write: Some(StatusResult::Success),
                },
            ],
        },
        Case {
            id: "partial-request-context",
            definition: &DEFINITION,
            fixture: Fixture {
                complete_context: false,
                ..repeated
            },
            expected: &[UNCERTAIN_EXPECTED],
        },
        Case {
            id: "unavailable-result-context",
            definition: &DEFINITION,
            fixture: Fixture {
                context_available: false,
                ..repeated
            },
            expected: &[],
        },
    ]
}

impl Fixture {
    pub fn batch(self) -> ToolResultBatch {
        // These types require one valid source envelope. This is not a parser fixture.
        let message = |id: &str, record_index, text: &str| UserMessage {
            source_format: SourceFormat::ClaudeJsonl,
            session_id: "synthetic".into(),
            message_id: id.into(),
            position: UserMessagePosition::Jsonl { record_index },
            origin: UserMessageOrigin::ClaudeUserContent,
            scope: UserMessageScope::MainSession,
            change: UserMessageChange::Present,
            completeness: UserMessageCompleteness::Complete,
            parts: vec![UserMessagePart {
                item_index: 0,
                native_part_id: None,
                text: text.into(),
            }],
            proof_revision: 1,
        };
        let mut requests = vec![message("request-1", 0, self.request)];
        let results = self
            .results
            .iter()
            .enumerate()
            .map(|(index, result)| {
                if index == 3
                    && let Some(text) = self.follow_up
                {
                    requests.push(message("request-2", 7, text));
                }
                let position = ToolResultPosition::Jsonl {
                    record_index: u64::try_from(index).expect("bounded fixture index") * 2 + 2,
                    item_index: 0,
                };
                ToolResultObservation {
                    facts: None,
                    outcome: None,
                    provider_executed: None,
                    source_format: SourceFormat::ClaudeJsonl,
                    session_id: "synthetic".into(),
                    record_id: Some(format!("record-{index}")),
                    call_id: Some(format!("call-{index}")),
                    result_id: Some(format!("result-{index}")),
                    position,
                    revision: 1,
                    change: ToolResultChange::Update,
                    tool_name: Some("Bash".into()),
                    tool_category: Some(ToolCategory::Bash),
                    arguments: Some(json!({"command": result.command})),
                    content: Some(result.content.into()),
                    status: if result.failed {
                        ToolResultStatus::Error
                    } else {
                        ToolResultStatus::Success
                    },
                    finality: true,
                    completeness: ToolResultCompleteness::Complete,
                    context: self.context_available.then(|| ToolResultContext {
                        result_position: position,
                        main_session: true,
                        complete_through_event: self.complete_context,
                        user_messages: requests.clone(),
                    }),
                }
            })
            .collect();
        ToolResultBatch {
            source_format: SourceFormat::ClaudeJsonl,
            results,
            complete: true,
        }
    }

    pub fn check_input(self, expected: &Expected, input: &Value) -> Result<()> {
        ensure!(
            input.as_object().context("input object")?.len() == 4,
            "unexpected input fields"
        );
        ensure!(
            input["request_history_complete"] == self.complete_context
                && input["source_coverage"] == "complete",
            "request-history quality differs from source context: history={}, coverage={}, expected_complete={}",
            input["request_history_complete"],
            input["source_coverage"],
            self.complete_context
        );
        let requests = input["requests"].as_array().context("request prefix")?;
        let mut texts = vec![self.request];
        if expected.trigger >= 4
            && let Some(text) = self.follow_up
        {
            texts.push(text);
        }
        ensure!(
            requests.len() == texts.len(),
            "request prefix includes missing or future messages"
        );
        for (index, (request, text)) in requests.iter().zip(texts).enumerate() {
            ensure!(
                *request
                    == json!({"id": format!("request-{}", index + 1), "source": "claude-code", "kind": "user_message", "text": text}),
                "request differs from the exact frozen prefix"
            );
        }
        let activity = input["activity"].as_array().context("activity window")?;
        ensure!(
            activity.len() == expected.window,
            "window differs from the declared prefix"
        );
        let end = usize::try_from(expected.trigger)?;
        for (offset, message) in activity.iter().enumerate() {
            let index = end - expected.window + offset;
            let result = &self.results[index];
            ensure!(
                message.as_object().context("activity message")?.len() == 7,
                "unexpected activity fields"
            );
            let identity: Value =
                serde_json::from_str(message["id"].as_str().context("message identity")?)?;
            ensure!(
                identity
                    == json!([
                        "synthetic",
                        format!("record-{index}"),
                        format!("call-{index}"),
                        format!("result-{index}")
                    ])
                    && message["source"] == "claude-code"
                    && message["kind"] == "tool_result"
                    && message["role"] == "tool",
                "message identity, source, kind, or role differs from session evidence"
            );
            ensure!(
                message["tool"]
                    == json!({
                        "name": "Bash", "category": "shell", "arguments": {"command": result.command},
                        "result": {"content": result.content, "status": if result.failed {"error"} else {"completed"}}
                    }),
                "tool arguments, result, status, or order differs from session evidence"
            );
            ensure!(
                message["quality"]
                    == json!({
                        "source_complete": true,
                        "tool_name_available": true,
                        "category_available": true,
                        "arguments_available": true,
                        "result_content_available": true,
                        "request_history_complete": self.complete_context,
                        "main_session": true,
                        "paths_available": false,
                        "paths_complete": false,
                        "paths_limits": []
                    }),
                "source-quality details differ from session evidence"
            );
            let position: ToolResultPosition =
                serde_json::from_str(message["native_order"].as_str().context("native order")?)?;
            ensure!(
                position
                    == ToolResultPosition::Jsonl {
                        record_index: u64::try_from(index)? * 2 + 2,
                        item_index: 0
                    },
                "native order differs from session evidence"
            );
        }
        Ok(())
    }
}
