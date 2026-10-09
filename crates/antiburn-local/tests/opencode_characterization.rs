use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, CoverageReason, EventSource, EvidenceSource, EvidenceValue, MemoryTurnRowStore,
    NormalizedRecord, PartialReason, RawSource, RecordSink, SessionCollector, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SessionSummary,
    SourceCapabilities, SourceKind, TurnContent, TurnRowSink, TurnRowStore, VisitOutcome,
    reader_for,
};
use antiburn_local::discovery::Explorers;
use antiburn_local::insights::{
    CoverageCounts, DetectorId, EfficiencyReportAccumulator, ReportContext, ReportWindow,
};
use antiburn_local::model::AgentKind;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use tempfile::TempDir;

fn create_database() -> (TempDir, std::path::PathBuf) {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).expect("database");
    connection
        .execute_batch(
            "CREATE TABLE session (
                 id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                 time_created INTEGER, time_updated INTEGER
             );
             CREATE TABLE message (
                 id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                 time_updated INTEGER, data TEXT
             );
             CREATE TABLE part (
                 id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                 time_created INTEGER, time_updated INTEGER, data TEXT
             );",
        )
        .expect("schema");
    drop(connection);
    (directory, path)
}

fn scope_sources(records: &str) -> (TempDir, Vec<SessionInput>) {
    let (directory, path) = create_database();
    let connection = Connection::open(&path).unwrap();
    insert_session(&connection, "root", None, None, 1000);
    let mut timestamp = 1000;
    for line in records.lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        match record["type"].as_str().unwrap() {
            "message" => {
                timestamp = record["time"]["created"].as_i64().unwrap();
                insert_message(
                    &connection,
                    record["messageID"].as_str().unwrap(),
                    "root",
                    timestamp,
                    &record["payload"].to_string(),
                );
            }
            "part" => insert_part(
                &connection,
                record["partID"].as_str().unwrap(),
                record["messageID"].as_str().unwrap(),
                "root",
                timestamp,
                &record["payload"].to_string(),
            ),
            "session_meta" => {}
            _ => panic!("unexpected fixture record"),
        }
    }
    drop(connection);
    let sqlite = SessionInput {
        source_format: antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2,
        ..sqlite_input(&path, "root")
    };
    let jsonl = SessionInput {
        source: RawSource::Jsonl(records.into()),
        source_format: antiburn_local::analysis::SourceFormat::OpenCodeJsonl,
        ..sqlite.clone()
    };
    (directory, vec![sqlite, jsonl])
}

#[test]
fn persisted_scope_field_availability_requires_supported_source_and_explicit_selection() {
    use antiburn_local::analysis::SourceFormat;
    use antiburn_local::analysis::jev::{
        JevFieldAvailabilityState, JevFieldCapability, JevInputField, JevInputSelection,
    };
    use antiburn_local::analysis::jev_evidence::{prepare_session_content, select_session_content};

    for observed in [true, false] {
        let records = include_str!("fixtures/opencode_characterization/scope_native.jsonl")
            .lines()
            .map(|line| {
                let mut record: serde_json::Value = serde_json::from_str(line).unwrap();
                if !observed && record["type"] == "part" {
                    if record["payload"]["type"] == "tool" {
                        record["payload"]["tool"] = json!("other");
                    } else {
                        record["payload"]["text"] = json!("Continue.");
                    }
                }
                record.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let (_directory, inputs) = scope_sources(&records);
        for input in inputs {
            let (_, store) = evidence_and_rows(&input);
            store.with_connection(|connection| {
                let query = |selection| {
                    let content = antiburn_local::analysis::query_turn_content_offset_selected(
                        connection,
                        &antiburn_local::analysis::TurnSessionKey {
                            environment_key: "native",
                            agent: "opencode",
                            session_id: "root",
                        },
                        &antiburn_local::analysis::FenceScope::single(1),
                        None,
                        &Default::default(),
                        0,
                        selection,
                    )
                    .unwrap();
                    select_session_content(
                        &prepare_session_content("root", input.source_format, content, Vec::new()),
                        selection,
                    )
                };
                let fields = [JevInputField::UserAnswer, JevInputField::PlanReference];
                let selected = query(JevInputSelection::from_fields(&fields));
                for field in fields {
                    let availability = selected
                        .field_availability
                        .iter()
                        .find(|a| a.field == field)
                        .unwrap();
                    assert!(availability.selected);
                    let (capability, state) = match (input.source_format, observed) {
                        (SourceFormat::OpenCodeSqliteV2, true) => (
                            JevFieldCapability::Conditional,
                            JevFieldAvailabilityState::Observed,
                        ),
                        (SourceFormat::OpenCodeSqliteV2, false) => (
                            JevFieldCapability::Conditional,
                            JevFieldAvailabilityState::NotObserved,
                        ),
                        (SourceFormat::OpenCodeJsonl, _) => (
                            JevFieldCapability::Unavailable,
                            JevFieldAvailabilityState::Unsupported,
                        ),
                        _ => unreachable!(),
                    };
                    assert_eq!(availability.capability, capability);
                    assert_eq!(availability.state, state);
                    assert_eq!(
                        availability.observed_parts > 0,
                        input.source_format == SourceFormat::OpenCodeSqliteV2 && observed
                    );
                }
                for selection in [
                    JevInputSelection::ALL,
                    antiburn_local::analysis::ignored_instructions::INPUT_SELECTION,
                ] {
                    let excluded = query(selection);
                    assert!(
                        excluded
                            .field_availability
                            .iter()
                            .all(|a| !fields.contains(&a.field))
                    );
                    assert!(
                        excluded
                            .actions
                            .iter()
                            .all(|a| a.metadata.user_answers.is_empty()
                                && a.metadata.plan_references.is_empty())
                    );
                }
            });
        }
    }
}

#[test]
fn scope_workflows_survive_storage_and_explicit_field_selection() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{
        JevPlanContentStatus, JevPlanStatus, JevUserAnswerOrigin, JevUserAnswerStatus,
    };
    let (_directory, inputs) = scope_sources(include_str!(
        "fixtures/opencode_characterization/scope_native.jsonl"
    ));
    for input in inputs {
        let (_, store) = evidence_and_rows(&input);
        store.with_connection(|connection| {
            let query = |selection| {
                antiburn_local::analysis::query_turn_content_offset_selected(
                    connection,
                    &antiburn_local::analysis::TurnSessionKey {
                        environment_key: "native",
                        agent: "opencode",
                        session_id: "root",
                    },
                    &antiburn_local::analysis::FenceScope::single(1),
                    None,
                    &Default::default(),
                    0,
                    selection,
                )
                .unwrap()
            };
            let selection = JevInputSelection::from_fields(&[
                JevInputField::UserAnswer,
                JevInputField::PlanReference,
            ]);
            let content = query(selection);
            let answers: Vec<_> = content
                .parts
                .iter()
                .flat_map(|p| &p.part.metadata.user_answers)
                .collect();
            assert_eq!(answers.len(), 6);
            assert!(
                content
                    .parts
                    .iter()
                    .filter(|p| p.part.tool_call_id.as_deref() == Some("call_interrupted"))
                    .all(|p| p.part.metadata.state
                        == antiburn_local::analysis::jev_evidence::JevOperationState::Error)
            );
            let calls: Vec<_> = answers
                .iter()
                .map(|a| a.source.call_id.as_deref().unwrap())
                .collect();
            let expected = if input.source_format
                == antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2
            {
                vec![
                    "call_fallback",
                    "call_interrupted",
                    "call_questions",
                    "call_questions",
                    "call_rejected",
                    "call_running",
                ]
            } else {
                vec![
                    "call_questions",
                    "call_questions",
                    "call_rejected",
                    "call_interrupted",
                    "call_running",
                    "call_fallback",
                ]
            };
            assert_eq!(calls, expected, "preserve each source's native part order");
            let scope = answers
                .iter()
                .find(|a| {
                    a.source.call_id.as_deref() == Some("call_questions") && a.source.order == 0
                })
                .unwrap();
            assert!(scope.is_authoritative_user_response());
            assert_eq!(
                scope.source.native_record_id.as_deref(),
                Some("prt_questions")
            );
            assert_eq!(scope.source.source_format, input.source_format);
            assert!(scope.source.question_id.is_none());
            assert_eq!(scope.multi_select, Some(true));
            assert_eq!(
                scope
                    .selections
                    .iter()
                    .map(|s| s.option_index)
                    .collect::<Vec<_>>(),
                vec![Some(0), Some(1)]
            );
            assert_eq!(
                scope.options[0].description.as_deref(),
                Some("Implement API only.\n  Do not change billing.")
            );
            let custom = answers
                .iter()
                .find(|a| {
                    a.source.call_id.as_deref() == Some("call_questions") && a.source.order == 1
                })
                .unwrap();
            assert!(custom.is_authoritative_user_response());
            assert_eq!(
                custom.free_text.as_deref(),
                Some("Well, no. Keep this:\n```\n  x = 2\n``` 实现 API only.")
            );
            assert_eq!(custom.selections[0].custom, Some(true));
            for (call, status) in [
                ("call_rejected", JevUserAnswerStatus::Cancelled),
                ("call_interrupted", JevUserAnswerStatus::Unknown),
                ("call_running", JevUserAnswerStatus::Pending),
            ] {
                let answer = answers
                    .iter()
                    .find(|a| a.source.call_id.as_deref() == Some(call))
                    .unwrap();
                assert_eq!(answer.status, status);
                assert!(!answer.is_authoritative_user_response());
                assert!(answer.selections.is_empty());
            }
            assert!(
                answers
                    .iter()
                    .find(|a| a.source.call_id.as_deref() == Some("call_fallback"))
                    .unwrap()
                    .is_authoritative_user_response()
            );
            let plans: Vec<_> = content
                .parts
                .iter()
                .flat_map(|p| &p.part.metadata.plan_references)
                .collect();
            assert_eq!(plans.len(), 2);
            assert_eq!(plans[0].source.call_id.as_deref(), Some("call_plan_exit"));
            assert_eq!(plans[0].origin, JevUserAnswerOrigin::User);
            assert_eq!(plans[0].status, JevPlanStatus::Approved);
            assert_eq!(plans[0].source.provenance, antiburn_local::analysis::jev_evidence::JevScopeEvidenceProvenance::RecognizedPlanWorkflow);
            assert_eq!(plans[1].origin, JevUserAnswerOrigin::Synthetic);
            assert_eq!(plans[1].status, JevPlanStatus::Unknown);
            assert_eq!(plans[1].source.provenance, antiburn_local::analysis::jev_evidence::JevScopeEvidenceProvenance::Unknown);
            assert!(plans[1].source.call_id.is_none());
            assert_eq!(
                plans[1].path.as_deref(),
                Some(".opencode/plans/1000-task.md")
            );
            for plan in plans {
                assert_eq!(plan.source.source_format, input.source_format);
                assert_eq!(plan.content_status, JevPlanContentStatus::Unresolved);
                assert!(
                    plan.text.is_none()
                        && plan.approved_revision.is_none()
                        && plan.approved_content_digest.is_none()
                );
            }
            let answer_only = query(JevInputSelection::from_fields(&[JevInputField::UserAnswer]));
            assert!(
                answer_only
                    .parts
                    .iter()
                    .all(|p| p.part.metadata.plan_references.is_empty() && p.part.text.is_empty())
            );
            let plan_only = query(JevInputSelection::from_fields(&[
                JevInputField::PlanReference,
            ]));
            assert!(
                plan_only
                    .parts
                    .iter()
                    .all(|p| p.part.metadata.user_answers.is_empty() && p.part.text.is_empty())
            );
            let legacy = query(antiburn_local::analysis::ignored_instructions::INPUT_SELECTION);
            assert!(
                legacy
                    .parts
                    .iter()
                    .all(|p| p.part.metadata.user_answers.is_empty()
                        && p.part.metadata.plan_references.is_empty())
            );
            let user = query(JevInputSelection::from_fields(&[
                JevInputField::UserMessage,
            ]));
            assert!(
                user.parts.iter().all(
                    |p| p.part.authority == antiburn_local::analysis::ContentAuthority::Unknown
                )
            );
            let user_selection = JevInputSelection::from_fields(&[JevInputField::UserMessage]);
            let selected_user = antiburn_local::analysis::jev_evidence::select_session_content(
                &antiburn_local::analysis::jev_evidence::prepare_session_content(
                    "root",
                    input.source_format,
                    user,
                    Vec::new(),
                ),
                user_selection,
            );
            assert!(
                selected_user.actions.is_empty(),
                "synthetic plan text is not ordinary user authority"
            );
            let selected = antiburn_local::analysis::jev_evidence::select_session_content(
                &antiburn_local::analysis::jev_evidence::prepare_session_content(
                    "root",
                    input.source_format,
                    content,
                    Vec::new(),
                ),
                selection,
            );
            assert_eq!(
                selected
                    .actions
                    .iter()
                    .any(|action| { !action.metadata.user_answers.is_empty() }),
                antiburn_local::analysis::jev_evidence::source_supported(input.source_format)
            );
            assert!(
                selected
                    .actions
                    .iter()
                    .all(|action| { action.text.is_empty() })
            );
        });
    }
}

#[test]
fn provider_executed_results_retain_unknown_scope_evidence_without_user_authority() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{
        JevPlanStatus, JevScopeEvidenceProvenance, JevUserAnswerOrigin, JevUserAnswerStatus,
    };

    for provider_executed in [false, true] {
        let mut records: Vec<serde_json::Value> =
            include_str!("fixtures/opencode_characterization/scope_native.jsonl")
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        for index in [2, 6, 8] {
            records[index]["payload"]["metadata"] = json!({"providerExecuted": provider_executed});
        }
        let records = records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let (_directory, inputs) = scope_sources(&records);
        for input in inputs {
            let (_, store) = evidence_and_rows(&input);
            store.with_connection(|connection| {
                let content = antiburn_local::analysis::query_turn_content_offset_selected(
                    connection,
                    &antiburn_local::analysis::TurnSessionKey {
                        environment_key: "native",
                        agent: "opencode",
                        session_id: "root",
                    },
                    &antiburn_local::analysis::FenceScope::single(1),
                    None,
                    &Default::default(),
                    0,
                    JevInputSelection::from_fields(&[
                        JevInputField::UserAnswer,
                        JevInputField::PlanReference,
                    ]),
                )
                .unwrap();
                let answers: Vec<_> = content
                    .parts
                    .iter()
                    .flat_map(|p| &p.part.metadata.user_answers)
                    .filter(|a| {
                        matches!(
                            a.source.call_id.as_deref(),
                            Some("call_questions" | "call_fallback")
                        )
                    })
                    .collect();
                assert_eq!(answers.len(), 3);
                for answer in &answers {
                    assert_eq!(answer.is_authoritative_user_response(), !provider_executed);
                    assert_eq!(
                        answer.status,
                        if provider_executed {
                            JevUserAnswerStatus::Unknown
                        } else {
                            JevUserAnswerStatus::Submitted
                        }
                    );
                    assert_eq!(
                        answer.origin,
                        if provider_executed {
                            JevUserAnswerOrigin::UnknownOrigin
                        } else {
                            JevUserAnswerOrigin::User
                        }
                    );
                    assert_eq!(
                        answer.source.provenance,
                        if provider_executed {
                            JevScopeEvidenceProvenance::Unknown
                        } else {
                            JevScopeEvidenceProvenance::RecognizedQuestionWorkflow
                        }
                    );
                    assert!(
                        !answer.selections.is_empty(),
                        "retain matched values even without user authority"
                    );
                    assert_eq!(answer.source.source_format, input.source_format);
                    assert!(answer.source.native_record_id.is_some());
                }
                let custom = answers
                    .iter()
                    .find(|a| {
                        a.source.call_id.as_deref() == Some("call_questions") && a.source.order == 1
                    })
                    .unwrap();
                assert_eq!(
                    custom.free_text.as_deref(),
                    Some("Well, no. Keep this:\n```\n  x = 2\n``` 实现 API only.")
                );
                let plans: Vec<_> = content
                    .parts
                    .iter()
                    .flat_map(|p| &p.part.metadata.plan_references)
                    .filter(|p| p.source.call_id.as_deref() == Some("call_plan_exit"))
                    .collect();
                assert_eq!(plans.len(), 1);
                let plan = plans[0];
                assert_eq!(
                    plan.status,
                    if provider_executed {
                        JevPlanStatus::Unknown
                    } else {
                        JevPlanStatus::Approved
                    }
                );
                assert_eq!(
                    plan.origin,
                    if provider_executed {
                        JevUserAnswerOrigin::UnknownOrigin
                    } else {
                        JevUserAnswerOrigin::User
                    }
                );
                assert_eq!(
                    plan.source.provenance,
                    if provider_executed {
                        JevScopeEvidenceProvenance::Unknown
                    } else {
                        JevScopeEvidenceProvenance::RecognizedPlanWorkflow
                    }
                );
                assert_eq!(
                    plan.source.native_record_id.as_deref(),
                    Some("prt_plan_exit")
                );
            });
        }
    }
}

#[test]
fn synthetic_attachment_approval_text_never_binds_to_a_plan_workflow() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{
        JevPlanContentStatus, JevPlanStatus, JevScopeEvidenceProvenance, JevUserAnswerOrigin,
        prepare_session_content, select_session_content,
    };

    for nearby_plan_exit in [false, true] {
        let attachments =
            include_str!("fixtures/opencode_characterization/synthetic_attachments_native.jsonl");
        let records = if nearby_plan_exit {
            format!(
                "{}{}",
                include_str!("fixtures/opencode_characterization/scope_native.jsonl"),
                attachments.lines().skip(1).collect::<Vec<_>>().join("\n")
            )
        } else {
            attachments.into()
        };
        let (_directory, inputs) = scope_sources(&records);
        for input in inputs {
            let (_, store) = evidence_and_rows(&input);
            store.with_connection(|connection| {
                let query = |selection| {
                    antiburn_local::analysis::query_turn_content_offset_selected(
                        connection,
                        &antiburn_local::analysis::TurnSessionKey {
                            environment_key: "native",
                            agent: "opencode",
                            session_id: "root",
                        },
                        &antiburn_local::analysis::FenceScope::single(1),
                        None,
                        &Default::default(),
                        0,
                        selection,
                    )
                    .unwrap()
                };
                let content = query(JevInputSelection::from_fields(&[
                    JevInputField::PlanReference,
                ]));
                let plans: Vec<_> = content
                    .parts
                    .iter()
                    .flat_map(|p| &p.part.metadata.plan_references)
                    .filter(|p| p.origin == JevUserAnswerOrigin::Synthetic)
                    .collect();
                assert_eq!(plans.len(), if nearby_plan_exit { 3 } else { 2 });
                for plan in plans {
                    assert_eq!(plan.status, JevPlanStatus::Unknown);
                    assert_eq!(plan.source.provenance, JevScopeEvidenceProvenance::Unknown);
                    assert!(plan.source.call_id.is_none());
                    assert_eq!(plan.path.as_deref(), Some(".opencode/plans/1000-task.md"));
                    assert_eq!(plan.content_status, JevPlanContentStatus::Unresolved);
                    assert!(
                        plan.text.is_none()
                            && plan.approved_content_digest.is_none()
                            && plan.approved_revision.is_none()
                    );
                }
                let selection = JevInputSelection::from_fields(&[JevInputField::UserMessage]);
                let user = select_session_content(
                    &prepare_session_content(
                        "root",
                        input.source_format,
                        query(selection),
                        Vec::new(),
                    ),
                    selection,
                );
                assert!(
                    user.actions.is_empty(),
                    "attachments do not become authoritative user messages"
                );
            });
        }
    }
}

#[test]
fn question_workflows_do_not_promote_incomplete_or_unmatched_results() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    for case in [
        "wrong_tool",
        "missing_call",
        "wrong_message",
        "wrong_session",
        "missing_prompt",
        "missing_description",
        "malformed_multiple",
        "malformed_answers",
        "missing_answers",
        "missing_output",
        "mismatched_output",
        "too_few_answers",
        "too_many_answers",
        "empty_answers",
        "truncated",
        "compacted",
        "interrupted",
        "pending",
        "running",
        "error",
        "user_role",
        "oversized_metadata",
        "missing_time",
        "wrong_title",
    ] {
        let mut records: Vec<serde_json::Value> =
            include_str!("fixtures/opencode_characterization/scope_native.jsonl")
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        match case {
            "wrong_tool" => records[2]["payload"]["tool"] = json!("other_question"),
            "missing_call" => records[2]["payload"]["callID"] = json!(null),
            "wrong_message" => records[2]["payload"]["messageID"] = json!("other_message"),
            "wrong_session" => records[2]["payload"]["sessionID"] = json!("other_session"),
            "missing_prompt" => {
                records[2]["payload"]["state"]["input"]["questions"][0]["question"] = json!(null)
            }
            "missing_description" => {
                records[2]["payload"]["state"]["input"]["questions"][0]["options"][0]["description"] =
                    json!(null)
            }
            "malformed_multiple" => {
                records[2]["payload"]["state"]["input"]["questions"][0]["multiple"] = json!("true")
            }
            "malformed_answers" => {
                records[2]["payload"]["state"]["metadata"]["answers"] = json!({"0":"API"})
            }
            "missing_answers" => {
                records[2]["payload"]["state"]["metadata"]
                    .as_object_mut()
                    .unwrap()
                    .remove("answers");
            }
            "missing_output" => records[2]["payload"]["state"]["output"] = json!(null),
            "mismatched_output" => records[2]["payload"]["state"]["output"] = json!("approved"),
            "too_few_answers" => {
                records[2]["payload"]["state"]["metadata"]["answers"] = json!([["API"]])
            }
            "too_many_answers" => {
                records[2]["payload"]["state"]["metadata"]["answers"] =
                    json!([["API"], ["Tests"], ["Yes"]])
            }
            "empty_answers" => {
                records[2]["payload"]["state"]["metadata"]["answers"] = json!([[], []]);
                records[2]["payload"]["state"]["output"] = json!(
                    "User has answered your questions: \"Which changes?\"=\"Unanswered\", \"Any conditions?\"=\"Unanswered\". You can now continue with the user's answers in mind."
                );
            }
            "truncated" => records[2]["payload"]["state"]["metadata"]["truncated"] = json!(true),
            "compacted" => records[2]["payload"]["state"]["time"]["compacted"] = json!(1020),
            "interrupted" => {
                records[2]["payload"]["state"]["metadata"]["interrupted"] = json!(true)
            }
            "pending" | "running" | "error" => {
                records[2]["payload"]["state"]["status"] = json!(case)
            }
            "user_role" => records[1]["payload"]["role"] = json!("user"),
            "oversized_metadata" => {
                records[2]["payload"]["state"]["input"]["questions"][0]["options"][0]["description"] =
                    json!("x".repeat(antiburn_local::analysis::MAX_CONTENT_PART_BYTES))
            }
            "missing_time" => records[2]["payload"]["state"]["time"] = json!(null),
            "wrong_title" => records[2]["payload"]["state"]["title"] = json!("Asked 1 question"),
            _ => unreachable!(),
        }
        let records = records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let (_directory, inputs) = scope_sources(&records);
        for input in inputs {
            let (_, store) = evidence_and_rows(&input);
            store.with_connection(|connection| {
                let content = antiburn_local::analysis::query_turn_content_offset_selected(
                    connection,
                    &antiburn_local::analysis::TurnSessionKey {
                        environment_key: "native",
                        agent: "opencode",
                        session_id: "root",
                    },
                    &antiburn_local::analysis::FenceScope::single(1),
                    None,
                    &Default::default(),
                    0,
                    JevInputSelection::from_fields(&[JevInputField::UserAnswer]),
                )
                .unwrap();
                for answer in content
                    .parts
                    .iter()
                    .flat_map(|p| &p.part.metadata.user_answers)
                    .filter(|a| a.source.call_id.as_deref() == Some("call_questions"))
                {
                    assert!(
                        !answer.is_authoritative_user_response(),
                        "{case}: {:?}",
                        input.source_format
                    );
                    assert!(answer.selections.is_empty(), "{case}");
                }
            });
        }
    }
}

#[test]
fn plan_workflows_require_native_shapes_and_keep_versions_unavailable() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{JevPlanStatus, JevUserAnswerOrigin};
    for case in [
        "wrong_tool",
        "assistant_plan",
        "wrong_input",
        "wrong_output",
        "wrong_title",
        "wrong_metadata",
        "empty_metadata",
        "truncated",
        "missing_call",
        "interrupted",
        "running",
        "rejected",
        "not_synthetic",
        "wrong_agent",
        "missing_model",
        "assistant_message",
        "wrong_synthetic_message",
        "wrong_synthetic_session",
        "ordinary_synthetic",
    ] {
        let mut records: Vec<serde_json::Value> =
            include_str!("fixtures/opencode_characterization/scope_native.jsonl")
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
        match case {
            "wrong_tool" => records[8]["payload"]["tool"] = json!("write"),
            "assistant_plan" => {
                records[8]["payload"] =
                    json!({"type":"text","text":"Plan: Implement billing. Approved."})
            }
            "wrong_input" => {
                records[8]["payload"]["state"]["input"] = json!({"plan":"Implement billing"})
            }
            "wrong_output" => records[8]["payload"]["state"]["output"] = json!("Approved"),
            "wrong_title" => records[8]["payload"]["state"]["title"] = json!("Approved"),
            "wrong_metadata" => {
                records[8]["payload"]["state"]["metadata"] = json!({"answers":[["Yes"]]})
            }
            "empty_metadata" => records[8]["payload"]["state"]["metadata"] = json!({}),
            "truncated" => records[8]["payload"]["state"]["metadata"]["truncated"] = json!(true),
            "missing_call" => records[8]["payload"]["callID"] = json!(null),
            "interrupted" => {
                records[8]["payload"]["state"]["metadata"] = json!({"interrupted":true})
            }
            "running" => records[8]["payload"]["state"]["status"] = json!("running"),
            "rejected" => {
                records[8]["payload"]["state"]["status"] = json!("error");
                records[8]["payload"]["state"]["error"] = json!("The user dismissed this question");
            }
            "not_synthetic" => records[10]["payload"]["synthetic"] = json!(false),
            "wrong_agent" => records[9]["payload"]["agent"] = json!("plan"),
            "missing_model" => records[9]["payload"]["model"] = json!(null),
            "assistant_message" => records[9]["payload"]["role"] = json!("assistant"),
            "wrong_synthetic_message" => {
                records[10]["payload"]["messageID"] = json!("other_message")
            }
            "wrong_synthetic_session" => {
                records[10]["payload"]["sessionID"] = json!("other_session")
            }
            "ordinary_synthetic" => records[10]["payload"]["text"] = json!("Plan approved"),
            _ => unreachable!(),
        }
        let records = records
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let (_directory, inputs) = scope_sources(&records);
        for input in inputs {
            let (_, store) = evidence_and_rows(&input);
            store.with_connection(|connection| {
                let content = antiburn_local::analysis::query_turn_content_offset_selected(
                    connection,
                    &antiburn_local::analysis::TurnSessionKey {
                        environment_key: "native",
                        agent: "opencode",
                        session_id: "root",
                    },
                    &antiburn_local::analysis::FenceScope::single(1),
                    None,
                    &Default::default(),
                    0,
                    JevInputSelection::from_fields(&[JevInputField::PlanReference]),
                )
                .unwrap();
                let plans: Vec<_> = content
                    .parts
                    .iter()
                    .flat_map(|p| &p.part.metadata.plan_references)
                    .collect();
                if matches!(
                    case,
                    "not_synthetic"
                        | "wrong_agent"
                        | "missing_model"
                        | "assistant_message"
                        | "wrong_synthetic_message"
                        | "wrong_synthetic_session"
                        | "ordinary_synthetic"
                ) {
                    assert!(
                        plans
                            .iter()
                            .all(|p| p.origin != JevUserAnswerOrigin::Synthetic),
                        "{case}"
                    );
                } else {
                    assert!(
                        plans
                            .iter()
                            .filter(|p| p.source.call_id.as_deref() == Some("call_plan_exit"))
                            .all(|p| p.status != JevPlanStatus::Approved),
                        "{case}"
                    );
                }
                if case == "rejected" {
                    assert_eq!(
                        plans
                            .iter()
                            .find(|p| p.source.call_id.as_deref() == Some("call_plan_exit"))
                            .unwrap()
                            .status,
                        JevPlanStatus::Cancelled
                    );
                }
                assert!(plans.iter().all(|p| p.text.is_none()
                    && p.content_digest.is_none()
                    && p.approved_content_digest.is_none()));
            });
        }
    }
}

fn sqlite_input(path: &std::path::Path, session_id: &str) -> SessionInput {
    SessionInput {
        agent: "opencode".to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::Sqlite(path.to_owned()),
        fork_parent_session_id: None,
        source_format: Default::default(),
    }
}

#[test]
fn opencode_formats_keep_distinct_native_source_contracts() {
    use antiburn_local::analysis::SourceFormat;

    let (_directory, path) = create_database();
    let sqlite = sqlite_input(&path, "root");
    assert_eq!(
        reader_for("opencode").capabilities(&sqlite).source_format,
        SourceFormat::OpenCodeSqliteV2
    );

    let jsonl = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Jsonl(
            r#"{"type":"message","sessionID":"root","messageID":"m1","time":{"created":1000},"payload":{"role":"assistant","modelID":"model-a","tokens":{"input":2,"output":3}}}
{"type":"part","messageID":"m1","payload":{"type":"text","text":"response"}}"#.to_owned(),
        ),
        fork_parent_session_id: None,
        source_format: SourceFormat::OpenCodeJsonl,
    };
    assert_eq!(
        reader_for("opencode").capabilities(&jsonl).source_format,
        SourceFormat::OpenCodeJsonl
    );
    let session = reader_for("opencode")
        .normalize(&jsonl)
        .expect("normalize exported JSONL");
    assert_eq!(session.events.len(), 1);
    assert_eq!(session.events[0].usage.input_tokens, 2);

    let message_jsonl = SessionInput {
        source: RawSource::Jsonl(
            r#"{"role":"assistant","time":{"created":1000},"modelID":"model-a"}"#.to_owned(),
        ),
        ..jsonl
    };
    let session = reader_for("opencode")
        .normalize(&message_jsonl)
        .expect("normalize unsupported shape as partial evidence");
    assert!(session.events.is_empty());
}

#[test]
fn sqlite_tool_lifecycle_keeps_requests_distinct_from_results() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 1_000);
    let records = include_str!("fixtures/opencode_characterization/tool_lifecycle_native.jsonl");
    for line in records.lines() {
        let record: serde_json::Value = serde_json::from_str(line).expect("native record");
        match record["type"].as_str().expect("record type") {
            "message" => insert_message(
                &connection,
                record["messageID"].as_str().expect("message id"),
                "root",
                1_000,
                &record["payload"].to_string(),
            ),
            "part" => insert_part(
                &connection,
                record["payload"]["callID"].as_str().expect("call id"),
                record["messageID"].as_str().expect("message id"),
                "root",
                1_001,
                &record["payload"].to_string(),
            ),
            _ => panic!("unexpected native record"),
        }
    }
    drop(connection);

    let session = reader_for("opencode")
        .normalize(&sqlite_input(&path, "root"))
        .expect("normalize native SQLite fixture");
    let calls = &session.events[0].tools;
    assert_eq!(calls.len(), 4);
    assert!(calls.iter().all(|call| call.name == "bash"));

    let input = sqlite_input(&path, "root");
    let mut capture = ContentCapturingSink::default();
    reader_for("opencode")
        .visit(&input, &mut capture)
        .expect("capture lifecycle content");
    let parts = &capture.contents[0].parts;
    let inputs: Vec<_> = parts
        .iter()
        .filter(|part| part.kind == antiburn_local::analysis::ContentKind::ToolInput)
        .collect();
    let outputs: Vec<_> = parts
        .iter()
        .filter(|part| part.kind == antiburn_local::analysis::ContentKind::ToolResult)
        .collect();
    assert_eq!(inputs.len(), 4);
    assert_eq!(outputs.len(), 2);
    for (call, command) in [
        ("call-completed", "printf completed"),
        ("call-running", "printf running"),
        ("call-error", "printf error"),
        ("call-pending", "printf pending"),
    ] {
        let part = inputs
            .iter()
            .find(|part| part.tool_call_id.as_deref() == Some(call))
            .expect("exact call identity");
        assert_eq!(
            part.authority,
            antiburn_local::analysis::ContentAuthority::Assistant
        );
        assert_eq!(
            part.normalized_fields.as_ref().unwrap().values
                [&antiburn_local::analysis::jev::JevInputField::BashCommandInput],
            command
        );
    }
    assert_eq!(outputs[0].tool_call_id.as_deref(), Some("call-completed"));
    assert_eq!(outputs[0].text, "completed output");
    assert_eq!(outputs[1].tool_call_id.as_deref(), Some("call-error"));
    assert_eq!(outputs[1].text, "command failed");
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::JevOperationState;
    let (_, store) = evidence_and_rows(&input);
    store.with_connection(|connection| {
        let selection = JevInputSelection::from_fields(&[JevInputField::BashCommandInput]);
        let published = antiburn_local::analysis::query_turn_content_offset_selected(
            connection,
            &antiburn_local::analysis::TurnSessionKey {
                environment_key: "native",
                agent: "opencode",
                session_id: "root",
            },
            &antiburn_local::analysis::FenceScope::single(1),
            None,
            &Default::default(),
            0,
            selection,
        )
        .unwrap();
        let selected = antiburn_local::analysis::jev_evidence::select_session_content(
            &antiburn_local::analysis::jev_evidence::prepare_session_content(
                "root",
                antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2,
                published,
                Vec::new(),
            ),
            selection,
        );
        assert_eq!(selected.actions.len(), 4);
        for (call, expected) in [
            ("call-completed", JevOperationState::Completed),
            ("call-running", JevOperationState::Running),
            ("call-error", JevOperationState::Error),
            ("call-pending", JevOperationState::Pending),
        ] {
            let action = selected
                .actions
                .iter()
                .find(|action| action.tool_call_id.as_deref() == Some(call))
                .unwrap();
            assert_eq!(action.metadata.state, expected);
            assert_eq!(action.metadata.bindings.len(), 1);
            let binding = &action.metadata.bindings[0];
            assert_eq!(binding.field, JevInputField::BashCommandInput);
            assert_eq!(binding.pointer, "/state/input/command");
            assert_eq!((binding.start, binding.end), (0, action.text.len()));
            assert!(!action.truncated);
            assert!(!action.text.contains("output"));
            assert!(!action.text.contains("failed"));
        }
    });
    assert!(
        outputs
            .iter()
            .all(|part| part.authority == antiburn_local::analysis::ContentAuthority::Tool)
    );

    let (_, store) = evidence_and_rows(&input);
    store.with_connection(|connection| {
        use antiburn_local::analysis::ignored_instructions::{
            INPUT_SELECTION, prepare_session_content, select_session_content,
        };
        use antiburn_local::analysis::{
            FenceScope, TurnSessionKey, query_turn_content_offset_selected,
        };
        let published = query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "opencode",
                session_id: "root",
            },
            &FenceScope::single(1),
            None,
            &std::collections::BTreeMap::new(),
            0,
            INPUT_SELECTION,
        )
        .unwrap();
        let selected = select_session_content(
            &prepare_session_content(
                "root",
                antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2,
                published,
                Vec::new(),
            ),
            INPUT_SELECTION,
        );
        use antiburn_local::analysis::jev_evidence::JevNativeFieldContainer;
        let expected = [
            (
                "call-completed",
                "tool_input",
                JevOperationState::Completed,
                JevInputField::BashCommandInput,
                "/state/input/command",
                "printf completed",
            ),
            (
                "call-completed",
                "tool_result",
                JevOperationState::Completed,
                JevInputField::BashCommandOutput,
                "/state/output",
                "completed output",
            ),
            (
                "call-running",
                "tool_input",
                JevOperationState::Running,
                JevInputField::BashCommandInput,
                "/state/input/command",
                "printf running",
            ),
            (
                "call-error",
                "tool_input",
                JevOperationState::Error,
                JevInputField::BashCommandInput,
                "/state/input/command",
                "printf error",
            ),
            (
                "call-error",
                "tool_result",
                JevOperationState::Error,
                JevInputField::BashCommandOutput,
                "/state/error",
                "command failed",
            ),
            (
                "call-pending",
                "tool_input",
                JevOperationState::Pending,
                JevInputField::BashCommandInput,
                "/state/input/command",
                "printf pending",
            ),
        ];
        assert_eq!(selected.actions.len(), expected.len());
        let native: Vec<Value> = records
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for (call, kind, state, field, pointer, text) in expected {
            let matching: Vec<_> = selected
                .actions
                .iter()
                .filter(|action| {
                    action.tool_call_id.as_deref() == Some(call) && action.kind == kind
                })
                .collect();
            assert_eq!(matching.len(), 1, "{call}: {kind}");
            let action = matching[0];
            assert_eq!(action.tool_name.as_deref(), Some("bash"));
            assert_eq!(action.turn_role, "assistant");
            assert_eq!(
                action.authority,
                if kind == "tool_input" {
                    "assistant"
                } else {
                    "tool"
                }
            );
            assert_eq!(action.metadata.state, state);
            assert_eq!(action.text, text);
            assert!(!action.truncated);
            assert!(action.metadata.human_text.is_none());
            assert!(action.metadata.user_text_history.is_none());
            assert_eq!(action.metadata.bindings.len(), 1);
            let range = &action.metadata.bindings[0];
            assert_eq!(range.field, field);
            assert_eq!(range.container, JevNativeFieldContainer::Part);
            assert_eq!(range.native_record_id.as_deref(), Some("m-lifecycle"));
            assert_eq!(range.pointer, pointer);
            assert_eq!((range.start, range.end), (0, text.len()));
            let payload = &native
                .iter()
                .find(|record| record["payload"]["callID"] == call)
                .unwrap()["payload"];
            assert_eq!(
                payload
                    .pointer(pointer)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .get(range.start..range.end),
                Some(action.text.as_str())
            );
            if kind == "tool_result" {
                let fact = action
                    .metadata
                    .command_result
                    .as_ref()
                    .expect("native bound command result");
                let request = selected
                    .actions
                    .iter()
                    .find(|request| {
                        request.kind == "tool_input"
                            && request.tool_call_id.as_deref() == Some(call)
                    })
                    .unwrap();
                assert!(fact.matches_action(action));
                assert!(fact.matches_request(request));
                assert_eq!(fact.state, state);
                assert_eq!(fact.call_id, call);
                assert_eq!(fact.range.pointer, pointer);
                assert_eq!(fact.request_range.pointer, "/state/input/command");
            } else {
                assert!(action.metadata.command_result.is_none());
            }
        }
    });
}

fn insert_session(
    connection: &Connection,
    id: &str,
    parent: Option<&str>,
    title: Option<&str>,
    timestamp: i64,
) {
    connection
        .execute(
            "INSERT INTO session (id, parent_id, title, time_created, time_updated)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, parent, title, timestamp],
        )
        .expect("session");
}

#[test]
fn sqlite_human_history_binds_exact_decoded_text_and_rejects_injected_or_wrong_identity() {
    use antiburn_local::analysis::SourceFormat;
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{
        JevNativeFieldContainer, prepare_session_content,
    };
    for control in [
        "native",
        "synthetic",
        "ignored",
        "wrong-message",
        "wrong-session",
    ] {
        let (_directory, path) = create_database();
        let connection = Connection::open(&path).unwrap();
        insert_session(&connection, "root", None, None, 1_000);
        insert_message(&connection, "human", "root", 1_000, &json!({"role":"user", "agent":"build", "model":{"providerID":"provider", "modelID":"model"}}).to_string());
        let mut native = vec![
            json!({"type":"text", "id":"p-1", "messageID":"human", "sessionID":"root", "text":"é\n\"quoted\"\\path"}),
            json!({"type":"text", "id":"p-2", "messageID":"human", "sessionID":"root", "text":"Second block 🦀"}),
        ];
        match control {
            "synthetic" => native[1]["synthetic"] = json!(true),
            "ignored" => native[1]["ignored"] = json!(true),
            "wrong-message" => native[1]["messageID"] = json!("other"),
            "wrong-session" => native[1]["sessionID"] = json!("other"),
            "native" => {}
            _ => unreachable!(),
        }
        for part in &native {
            insert_part(
                &connection,
                part["id"].as_str().unwrap(),
                "human",
                "root",
                1_001,
                &part.to_string(),
            );
        }
        drop(connection);
        let (_, store) = evidence_and_rows(&sqlite_input(&path, "root"));
        store.with_connection(|connection| {
            let published = antiburn_local::analysis::query_turn_content_offset_selected(
                connection,
                &antiburn_local::analysis::TurnSessionKey {
                    environment_key: "native",
                    agent: "opencode",
                    session_id: "root",
                },
                &antiburn_local::analysis::FenceScope::single(1),
                None,
                &Default::default(),
                0,
                JevInputSelection::from_fields(&[JevInputField::UserMessage]),
            )
            .unwrap();
            let prepared =
                prepare_session_content("root", SourceFormat::OpenCodeSqliteV2, published, vec![]);
            if control != "native" {
                assert!(
                    prepared.actions.iter().all(|action| action
                        .metadata
                        .user_text_history
                        .is_none()
                        && action.metadata.human_text.is_none()),
                    "{control}"
                );
                return;
            }
            assert_eq!(prepared.actions.len(), native.len());
            for (action, part) in prepared.actions.iter().zip(&native) {
                assert_eq!(action.authority, "user");
                assert_eq!(action.turn_role, "user");
                assert_eq!(action.text, part["text"].as_str().unwrap());
                let proof = action.metadata.user_text_history.as_ref().unwrap();
                assert_eq!(proof.message_id, "human");
                assert_eq!(proof.session_id, "root");
                let fact = action
                    .metadata
                    .human_text
                    .as_ref()
                    .expect("bound human text");
                assert!(fact.matches_action(action));
                assert_eq!(fact.range.native_record_id.as_deref(), Some("human"));
                assert_eq!(fact.range.container, JevNativeFieldContainer::Part);
                assert_eq!(fact.range.field, JevInputField::UserMessage);
                assert_eq!(fact.range.pointer, "/text");
                assert_eq!((fact.range.start, fact.range.end), (0, action.text.len()));
                assert_eq!(
                    part["text"]
                        .as_str()
                        .unwrap()
                        .get(fact.range.start..fact.range.end),
                    Some(action.text.as_str())
                );
            }
        });
    }
}

#[test]
fn sqlite_command_results_abstain_for_incomplete_or_ambiguous_native_calls() {
    use antiburn_local::analysis::SourceFormat;
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{
        JevOperationState, prepare_session_content, select_session_content,
    };
    for control in [
        "completed",
        "pending",
        "running",
        "truncated",
        "interrupted",
        "compacted",
        "wrong-message",
        "duplicate-call",
    ] {
        let (_directory, path) = create_database();
        let connection = Connection::open(&path).unwrap();
        insert_session(&connection, "root", None, None, 1_000);
        insert_message(
            &connection,
            "command",
            "root",
            1_000,
            &json!({"role":"assistant"}).to_string(),
        );
        let mut part = json!({"type":"tool", "id":"p-command", "messageID":"command", "sessionID":"root", "tool":"bash", "callID":"call", "state":{"status":"completed", "input":{"command":"printf é"}, "output":"é\n\"quoted\""}});
        match control {
            "pending" | "running" => part["state"]["status"] = json!(control),
            "truncated" | "interrupted" => part["state"]["metadata"][control] = json!(true),
            "compacted" => part["state"]["time"]["compacted"] = json!(1_002),
            "wrong-message" => part["messageID"] = json!("other"),
            "completed" | "duplicate-call" => {}
            _ => unreachable!(),
        }
        insert_part(
            &connection,
            "p-command",
            "command",
            "root",
            1_001,
            &part.to_string(),
        );
        if control == "duplicate-call" {
            insert_part(
                &connection,
                "p-duplicate",
                "command",
                "root",
                1_002,
                &part.to_string(),
            );
        }
        drop(connection);
        let (_, store) = evidence_and_rows(&sqlite_input(&path, "root"));
        store.with_connection(|connection| {
            let published = antiburn_local::analysis::query_turn_content_offset_selected(
                connection,
                &antiburn_local::analysis::TurnSessionKey {
                    environment_key: "native",
                    agent: "opencode",
                    session_id: "root",
                },
                &antiburn_local::analysis::FenceScope::single(1),
                None,
                &Default::default(),
                0,
                JevInputSelection::from_fields(&[
                    JevInputField::BashCommandInput,
                    JevInputField::BashCommandOutput,
                ]),
            )
            .unwrap();
            let prepared =
                prepare_session_content("root", SourceFormat::OpenCodeSqliteV2, published, vec![]);
            let prepared = select_session_content(
                &prepared,
                JevInputSelection::from_fields(&[
                    JevInputField::BashCommandInput,
                    JevInputField::BashCommandOutput,
                ]),
            );
            if control != "completed" {
                assert!(
                    prepared
                        .actions
                        .iter()
                        .all(|action| action.metadata.command_result.is_none()),
                    "{control}"
                );
                return;
            }
            let result = prepared
                .actions
                .iter()
                .find(|action| action.kind == "tool_result")
                .unwrap();
            let request = prepared
                .actions
                .iter()
                .find(|action| action.kind == "tool_input")
                .unwrap();
            let fact = result
                .metadata
                .command_result
                .as_ref()
                .expect("native completed command result");
            assert_eq!(result.text, "é\n\"quoted\"");
            assert_eq!(fact.state, JevOperationState::Completed);
            assert!(fact.matches_action(result) && fact.matches_request(request));
            assert_eq!(fact.range.pointer, "/state/output");
            assert_eq!(
                part.pointer(&fact.range.pointer)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .get(fact.range.start..fact.range.end),
                Some(result.text.as_str())
            );
        });
    }
}

#[test]
fn legacy_shell_results_stay_unknown_while_native_sqlite_read_has_observed_extent() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::{
        JevOperationState, JevReadStatus, prepare_session_content,
    };
    use antiburn_local::analysis::{ContentKind, SourceFormat};
    let legacy = SessionInput {
        agent: "opencode".into(),
        session_id: "root".into(),
        source: RawSource::Jsonl(
            include_str!("fixtures/opencode_characterization/tool_lifecycle_native.jsonl").into(),
        ),
        source_format: SourceFormat::OpenCodeJsonl,
        fork_parent_session_id: None,
    };
    let mut captured = ContentCapturingSink::default();
    reader_for("opencode")
        .visit(&legacy, &mut captured)
        .unwrap();
    let results: Vec<_> = captured
        .contents
        .iter()
        .flat_map(|content| &content.parts)
        .filter(|part| part.kind == ContentKind::ToolResult)
        .collect();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|part| part.metadata.state == JevOperationState::Unknown
                && part.metadata.bindings.is_empty()
                && part.metadata.command_result.is_none())
    );

    let (_directory, path) = create_database();
    let connection = Connection::open(&path).unwrap();
    insert_session(&connection, "root", None, None, 1_000);
    insert_message(
        &connection,
        "read",
        "root",
        1_000,
        &json!({"role":"assistant"}).to_string(),
    );
    let mut native: Value = serde_json::from_str(
        include_str!("fixtures/read_characterization/opencode.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    native["type"] = json!("tool");
    insert_part(
        &connection,
        "first",
        "read",
        "root",
        1_001,
        &native.to_string(),
    );
    drop(connection);
    let (_, store) = evidence_and_rows(&sqlite_input(&path, "root"));
    store.with_connection(|connection| {
        let published = antiburn_local::analysis::query_turn_content_offset_selected(
            connection,
            &antiburn_local::analysis::TurnSessionKey {
                environment_key: "native",
                agent: "opencode",
                session_id: "root",
            },
            &antiburn_local::analysis::FenceScope::single(1),
            None,
            &Default::default(),
            0,
            JevInputSelection::from_fields(&[
                JevInputField::ReadFileRequest,
                JevInputField::ReadFileResult,
            ]),
        )
        .unwrap();
        let prepared =
            prepare_session_content("root", SourceFormat::OpenCodeSqliteV2, published, vec![]);
        let action = prepared
            .actions
            .iter()
            .find(|action| action.kind == "tool_result")
            .unwrap();
        assert_eq!(action.tool_call_id.as_deref(), Some("read-1"));
        assert_eq!(action.authority, "tool");
        assert_eq!(action.metadata.state, JevOperationState::Completed);
        assert_eq!(action.text, native["state"]["output"].as_str().unwrap());
        let result = action
            .metadata
            .read_result
            .as_ref()
            .expect("typed native read result");
        assert_eq!(result.status, JevReadStatus::Success);
        let extent = result.returned_extent.as_ref().unwrap();
        assert_eq!(
            (extent.offset, extent.limit, extent.end_inclusive),
            (Some(2), Some(2), Some(3))
        );
        assert!(result.request_reference_id.is_some());
        assert!(action.metadata.command_result.is_none());
    });
}

/// Streams `session_id` through the OpenCode adapter and the real evidence
/// and turn-row pipeline, the same path production uses. Returns the
/// published evidence plus the row store, so a test can also read rows back
/// directly with [`MemoryTurnRowStore::with_connection`].
fn evidence_and_rows(input: &SessionInput) -> (SessionEvidence, Arc<MemoryTurnRowStore>) {
    let metrics = SessionMetricsAccumulator::new(&input.agent, &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: reader_for("opencode").capabilities(input),
    });
    let store = MemoryTurnRowStore::new(&input.agent, &input.session_id);
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        &input.session_id,
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = reader_for("opencode")
        .visit(input, &mut sink)
        .expect("stream opencode source");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("published evidence");
    (evidence, store)
}

/// Reads back `(scope, thread_id, child_id)` for every row of one session,
/// in turn order, straight off [`MemoryTurnRowStore`]'s in-memory database.
fn turn_identities(
    store: &MemoryTurnRowStore,
    session_id: &str,
) -> Vec<(String, String, Option<String>)> {
    store.with_connection(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT scope, thread_id, child_id FROM turn
                  WHERE environment_key = 'native' AND agent = 'opencode'
                    AND session_id = ?1 AND claim_fence = 1
                  ORDER BY turn_index",
            )
            .expect("prepare");
        statement
            .query_map(params![session_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    })
}

#[derive(Default)]
struct ContentCapturingSink {
    contents: Vec<TurnContent>,
}

impl RecordSink for ContentCapturingSink {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::TurnContent(content) = record {
            self.contents.push(*content);
        }
    }

    fn finish(&mut self, _summary: SessionSummary) {}
}

fn observed<T: Clone>(value: &EvidenceValue<T>) -> T {
    match value {
        EvidenceValue::Complete(observed) => observed.clone(),
        EvidenceValue::Partial { observed, .. } => observed.clone(),
        EvidenceValue::Unsupported => panic!("evidence unexpectedly unsupported"),
    }
}

fn insert_message(connection: &Connection, id: &str, session_id: &str, timestamp: i64, data: &str) {
    connection
        .execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
            params![id, session_id, timestamp, data],
        )
        .expect("message");
}

fn insert_part(
    connection: &Connection,
    id: &str,
    message_id: &str,
    session_id: &str,
    timestamp: i64,
    data: &str,
) {
    connection
        .execute(
            "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            params![id, message_id, session_id, timestamp, data],
        )
        .expect("part");
}

#[test]
fn opencode_capabilities_match_the_observed_contract() {
    assert_eq!(
        SourceCapabilities::opencode(),
        SourceCapabilities {
            source_format: antiburn_local::analysis::SourceFormat::OpenCodeJsonl,
            request_context_tokens: true,
            cache_write_tokens: true,
            timestamps_and_order: true,
            tool_invocations: true,
            skill_inventory: false,
            mcp_inventory: false,
            tool_definitions: false,
            model_identity: true,
            token_classes: true,
            reasoning_effort_tier: false,
            fast_tier: false,
            service_tier: false,
            subagent_relationships: true,
            subagent_models: true,
            compaction_boundaries: true,
            thread_identity: true,
            record_identity: false,
            linear_record_order: true,
            quota_incidents: false,
            provider_incidents: false,
            harness_version: false,
            repeated_context_accounting: None,
        }
    );
}

#[test]
fn native_repeated_context_reaches_reports_without_capability_overrides() {
    use antiburn_local::analysis::RepeatedContextAccounting;

    for (provider, model, accounting) in [
        (
            "anthropic",
            "claude-sonnet-4",
            RepeatedContextAccounting::CacheWrite,
        ),
        ("openai", "gpt-5", RepeatedContextAccounting::UncachedInput),
    ] {
        for case in [
            "finding",
            "clean",
            "route_switch",
            "unknown_route",
            "compaction",
            "initial_compaction",
            "record_loss",
            "missing_time",
            "conflicting_time",
            "fork",
            "known_fork",
            "tied_time",
            "reversed_export",
            "unwrapped_export",
            "duplicate_export",
        ] {
            let (_directory, path) = create_database();
            let connection = Connection::open(&path).expect("database");
            let start = if case == "fork" { 15 } else { 1 };
            insert_session(&connection, "root", None, None, start);
            let mut export = vec![
                json!({"type":"session_meta","time":{"created":start},"payload":{"id":"root"}}),
            ];
            for index in 0..4 {
                let id = format!("m{index}");
                let timestamp = if case == "tied_time" {
                    10
                } else {
                    10 + index * 10
                };
                let mut message = if index == 0 {
                    json!({"role":"user"})
                } else {
                    let paid = if case == "finding" || case == "tied_time" {
                        1000
                    } else {
                        0
                    };
                    json!({"role":"assistant","parentID":"m0","modelID":model,"providerID":provider,
                        "tokens":{"input":if provider == "openai" { paid } else { 0 },"output":10,"reasoning":3,
                            "cache":{"read":1000,"write":if provider == "anthropic" { paid } else { 0 }}}})
                };
                if case == "route_switch" && index == 2 {
                    message["providerID"] = json!(if provider == "anthropic" {
                        "openai"
                    } else {
                        "anthropic"
                    });
                }
                if case == "unknown_route" && index > 0 {
                    message["providerID"] = json!("custom-proxy");
                }
                if case == "conflicting_time" && index == 2 {
                    message["time"] = json!({"created":99});
                }
                insert_message(&connection, &id, "root", timestamp, &message.to_string());
                export.push(json!({"type":"message","sessionID":"root","messageID":id,"time":{"created":timestamp},"payload":message}));
                if case == "missing_time" && index == 2 {
                    connection
                        .execute(
                            "UPDATE message SET time_created = NULL WHERE id = ?1",
                            [&id],
                        )
                        .expect("missing creation time");
                    export.last_mut().unwrap()["time"]["created"] = json!(null);
                }
                if case == "record_loss" && index == 2 {
                    connection
                        .execute("UPDATE message SET data = '{' WHERE id = ?1", [&id])
                        .expect("malformed message");
                    *export.last_mut().unwrap() =
                        json!({"type":"message","messageID":id,"payload":null});
                }
                if (case == "compaction" && index == 2)
                    || (case == "initial_compaction" && index == 0)
                {
                    let part = json!({"type":"compaction","auto":true});
                    insert_part(
                        &connection,
                        "compact",
                        &id,
                        "root",
                        timestamp,
                        &part.to_string(),
                    );
                    export.push(json!({"type":"part","messageID":id,"payload":part}));
                }
            }
            drop(connection);
            if case == "reversed_export" {
                export.swap(2, 3);
            }
            if case == "unwrapped_export" {
                export.remove(0);
            }
            if case == "duplicate_export" {
                export[4]["messageID"] = json!("m1");
            }
            for source in [
                RawSource::Sqlite(path.clone()),
                RawSource::Jsonl(
                    export
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            ] {
                let export_only_gap = matches!(source, RawSource::Jsonl(_))
                    && matches!(
                        case,
                        "reversed_export" | "unwrapped_export" | "duplicate_export"
                    );
                let input = SessionInput {
                    source,
                    fork_parent_session_id: (case == "known_fork").then(|| "parent".to_owned()),
                    ..sqlite_input(&path, "root")
                };
                let (evidence, store) = evidence_and_rows(&input);
                let cache = observed(&evidence.cache);
                let incomplete = export_only_gap
                    || matches!(
                        case,
                        "route_switch"
                            | "compaction"
                            | "initial_compaction"
                            | "record_loss"
                            | "missing_time"
                            | "conflicting_time"
                            | "fork"
                            | "known_fork"
                    );
                if case == "unknown_route" {
                    assert_eq!(cache.repeated_context, EvidenceValue::Unsupported);
                } else {
                    assert_eq!(
                        matches!(cache.repeated_context, EvidenceValue::Partial { .. }),
                        incomplete,
                        "{provider}: {case}: {:?}",
                        cache.repeated_context
                    );
                    let repeated = observed(&cache.repeated_context);
                    assert_eq!(repeated.accounting, accounting, "{provider}: {case}");
                    if matches!(case, "finding" | "tied_time") {
                        assert_eq!(repeated.pairs_considered, 2);
                        assert_eq!(repeated.repeated_tokens, 2000);
                        assert_eq!(repeated.paid_tokens, 3000);
                    }
                }
                store.with_connection(|connection| {
                    let route: (String, Option<String>, Option<String>, i64) = connection.query_row(
                        "SELECT provider, api, parent_uuid, output_tokens FROM turn WHERE role = 'assistant' ORDER BY turn_index LIMIT 1",
                        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    ).expect("request route");
                    assert_eq!(route.0, if case == "unknown_route" { "custom-proxy" } else { provider });
                    assert_eq!(route.1, None);
                    assert_eq!(route.2, None);
                    assert_eq!(route.3, 13);
                });
                let mut report = EfficiencyReportAccumulator::new();
                report.observe_session(evidence);
                let report = report.finish(ReportContext {
                    environment_key: "native".to_owned(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: 100,
                    },
                    computed_at_epoch: 100,
                    parser_revision: antiburn_local::analysis::PARSER_REVISION,
                    analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
                    evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    coverage: CoverageCounts::default(),
                });
                let counts = report.detectors[DetectorId::CacheChurn.index()];
                if matches!(case, "finding" | "tied_time") {
                    assert_eq!(counts.finding, 0, "{provider}: {case}");
                    assert_eq!(counts.clean, 0, "{provider}: {case}");
                    assert_eq!(counts.assessed, 0, "{provider}: {case}");
                    assert_eq!(counts.unavailable, 1, "{provider}: {case}");
                } else if incomplete || case == "unknown_route" {
                    assert_eq!(counts.clean, 0, "{provider}: {case}");
                    assert_eq!(counts.assessed, 0, "{provider}: {case}");
                } else {
                    assert_eq!(counts.clean, 1, "{provider}: {case}");
                }
            }
        }
    }
}

#[test]
fn native_anthropic_cache_episode_uses_ordered_message_identity() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "cache-session", None, None, 1_000);
    let messages = [
        ("u0", 1_000, json!({"role":"user"})),
        (
            "a0",
            1_001,
            json!({"role":"assistant","modelID":"claude-sonnet-4","providerID":"anthropic",
                "tokens":{"input":0,"output":10,"cache":{"read":10_000,"write":0}}}),
        ),
        ("u1", 302_000, json!({"role":"user"})),
        (
            "a1",
            302_001,
            json!({"role":"assistant","parentID":"u1","modelID":"claude-sonnet-4","providerID":"anthropic",
                "tokens":{"input":0,"output":10,"cache":{"read":0,"write":10_000}}}),
        ),
        ("u2", 302_002, json!({"role":"user"})),
        (
            "a2",
            302_003,
            json!({"role":"assistant","parentID":"u2","modelID":"claude-sonnet-4","providerID":"anthropic",
                "tokens":{"input":0,"output":10,"cache":{"read":10_000,"write":0}}}),
        ),
    ];
    for (id, timestamp, data) in messages {
        insert_message(
            &connection,
            id,
            "cache-session",
            timestamp,
            &data.to_string(),
        );
    }
    drop(connection);

    let (evidence, _) = evidence_and_rows(&sqlite_input(&path, "cache-session"));
    let cache = observed(&evidence.cache);
    let repeated = observed(&cache.repeated_context);
    assert_eq!(repeated.possible_rehydration_episodes, 1);

    let mut report = EfficiencyReportAccumulator::new();
    report.observe_session(evidence);
    let report = report.finish(ReportContext {
        environment_key: "native".to_owned(),
        window: ReportWindow {
            start_epoch: 0,
            end_epoch: 1_000,
        },
        computed_at_epoch: 1_000,
        parser_revision: antiburn_local::analysis::PARSER_REVISION,
        analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
        evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
        coverage: CoverageCounts::default(),
    });
    assert_eq!(report.detectors[DetectorId::CacheChurn.index()].finding, 1);
}

#[test]
fn completed_skill_results_publish_identity_without_inventory() {
    // Official packages/opencode/src/tool/skill.ts defines both pinned output formats.
    for (revision, base) in [
        (
            "ffc000de8e446c63d41a2e352d119d9ff43530d0",
            "file:///PRIVATE_DIR",
        ),
        ("ecbc6ccac85b3e8087b6445e584318419b9e2b34", "/PRIVATE_DIR"),
    ] {
        for case in [
            "complete",
            "pending",
            "running",
            "error",
            "missing_output",
            "missing_name",
            "wrong_name",
            "empty_body",
            "redacted",
            "truncated",
            "metadata_truncated",
            "compacted",
            "unclosed",
            "invocation_only",
            "user",
        ] {
            let output = format!(
                "<skill_content name=\"review\">\n# Skill: review\n\nPRIVATE_BODY\n\nBase directory for this skill: {base}\nRelative paths in this skill (e.g., scripts/, reference/) are relative to this base directory.\nNote: file list is sampled.\n\n<skill_files>\n<file>/PRIVATE_FILE</file>\n</skill_files>\n</skill_content>"
            );
            let mut part = json!({"type":"tool","tool":"skill","callID":"skill-call","state":{
                "status":"completed", "input":{"name":"review"},
                "metadata":{"name":"review","dir":"/PRIVATE_DIR","truncated":false},
                "time":{"start":11,"end":12}, "output":output
            }});
            match case {
                "pending" | "running" | "error" => part["state"]["status"] = json!(case),
                "missing_output" | "invocation_only" => part["state"]["output"] = json!(null),
                "missing_name" => part["state"]["metadata"]["name"] = json!(null),
                "wrong_name" => part["state"]["metadata"]["name"] = json!("other"),
                "empty_body" => part["state"]["output"] = json!(output.replace("PRIVATE_BODY", "")),
                "redacted" | "truncated" => {
                    part["state"]["output"] =
                        json!(output.replace("PRIVATE_BODY", &format!("[{case}]")))
                }
                "metadata_truncated" => part["state"]["metadata"]["truncated"] = json!(true),
                "compacted" => part["state"]["time"]["compacted"] = json!(20),
                "unclosed" => {
                    part["state"]["output"] = json!(output.replace("</skill_content>", ""))
                }
                _ => {}
            }
            let message = json!({"role":if case == "user" { "user" } else { "assistant" }, "modelID":"model-a", "tokens":{"input":1,"output":1}});
            let (_directory, path) = create_database();
            let connection = Connection::open(&path).expect("database");
            insert_session(&connection, "root", None, None, 10);
            insert_message(&connection, "m1", "root", 10, &message.to_string());
            insert_part(&connection, "p1", "m1", "root", 11, &part.to_string());
            drop(connection);
            let export = [
                json!({"type":"message","messageID":"m1","sessionID":"root","time":{"created":10},"payload":message}),
                json!({"type":"part","messageID":"m1","payload":part}),
            ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
            for source in [RawSource::Sqlite(path.clone()), RawSource::Jsonl(export)] {
                let input = SessionInput {
                    source,
                    ..sqlite_input(&path, "root")
                };
                let (evidence, _) = evidence_and_rows(&input);
                assert!(!evidence.capabilities.skill_inventory);
                if case == "complete" {
                    let sources = observed(&evidence.context_sources);
                    let expected = match &input.source {
                        RawSource::Sqlite(_) => EvidenceValue::Complete(()),
                        _ => EvidenceValue::Partial {
                            observed: (),
                            reason: CoverageReason::AttributionIncomplete,
                        },
                    };
                    assert_eq!(sources.skill_coverage, expected, "{revision}");
                    assert_eq!(sources.skills.len(), 1);
                    let skill = &sources.skills["review"];
                    assert!(skill.injected && skill.invoked);
                    assert!(skill.description.is_none());
                    assert!(skill.token_count.is_none());
                    assert_eq!(skill.origin, EvidenceValue::Unsupported);
                } else {
                    assert_eq!(
                        evidence.context_sources,
                        EvidenceValue::Unsupported,
                        "{revision}: {case}"
                    );
                }
                let session = reader_for("opencode").normalize(&input).expect("normalize");
                let retained = format!("{}{}", json!(evidence), json!(session));
                for private in ["PRIVATE_BODY", "PRIVATE_DIR", "PRIVATE_FILE"] {
                    assert!(!retained.contains(private), "{revision}: {case}: {private}");
                }
                let mut report = EfficiencyReportAccumulator::new();
                report.observe_session(evidence);
                let report = report.finish(ReportContext {
                    environment_key: "native".to_owned(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: 100,
                    },
                    computed_at_epoch: 100,
                    parser_revision: antiburn_local::analysis::PARSER_REVISION,
                    analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
                    evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                    coverage: CoverageCounts::default(),
                });
                let counts = report.detectors[DetectorId::UnusedSkills.index()];
                assert_eq!(counts.finding, 0, "{revision}: {case}");
                assert_eq!(counts.assessed, 0, "{revision}: {case}");
                assert_eq!(counts.clean, 0, "{revision}: {case}");
            }
        }
    }
}

#[test]
fn native_sqlite_streams_root_and_descendant_messages_in_order() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 10);
    insert_session(&connection, "child", Some("root"), None, 20);
    insert_message(
        &connection,
        "later",
        "root",
        40,
        r#"{"role":"assistant","modelID":"model-a","variant":"high","tokens":{"input":100,"output":20,"reasoning":5,"cache":{"read":30,"write":40}}}"#,
    );
    insert_message(&connection, "earlier", "child", 30, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "child-assistant",
        "child",
        35,
        r#"{"role":"assistant","modelID":"model-child","tokens":{"input":3,"output":2}}"#,
    );
    insert_part(
        &connection,
        "child-tool",
        "child-assistant",
        "child",
        36,
        r#"{"type":"tool","tool":"read","state":{"input":{}}}"#,
    );
    insert_part(
        &connection,
        "reasoning",
        "later",
        "root",
        41,
        r#"{"type":"reasoning","text":"PRIVATE_REASONING"}"#,
    );
    insert_part(
        &connection,
        "tool",
        "later",
        "root",
        42,
        r#"{"type":"tool","tool":"bash","state":{"input":{"command":"cargo test PRIVATE_PATH"},"output":"PRIVATE_OUTPUT"}}"#,
    );
    insert_part(
        &connection,
        "patch",
        "later",
        "root",
        43,
        r#"{"type":"patch","files":["PRIVATE_PATH"],"diff":"PRIVATE_DIFF"}"#,
    );
    insert_part(
        &connection,
        "compaction",
        "later",
        "root",
        44,
        r#"{"type":"compaction","auto":true,"snapshot":"PRIVATE_SNAPSHOT"}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    let session = collector.into_session().expect("finished session");

    assert_eq!(session.events.len(), 3);
    assert_eq!(session.events[0].role, antiburn_local::analysis::Role::User);
    // Ancestry separates child work but does not prove a native spawn.
    assert_eq!(session.events[0].thread_id.as_deref(), Some("child"));
    assert_eq!(session.events[0].source, EventSource::Subagent);
    let child_assistant = &session.events[1];
    assert_eq!(child_assistant.model.as_deref(), Some("model-child"));
    assert_eq!(child_assistant.tools.len(), 1);
    assert_eq!(child_assistant.thread_id.as_deref(), Some("child"));
    assert_eq!(child_assistant.source, EventSource::Subagent);
    let assistant = &session.events[2];
    assert_eq!(assistant.thinking_mode, None);
    assert_eq!(assistant.usage.input_tokens, 100);
    assert_eq!(assistant.usage.output_tokens, 25);
    assert_eq!(assistant.usage.cache_read_tokens, 30);
    assert_eq!(assistant.usage.cache_creation_tokens, 40);
    assert_eq!(assistant.model.as_deref(), Some("model-a"));
    assert_eq!(assistant.thinking_mode, None);
    assert!(assistant.has_thinking);
    assert!(assistant.is_compaction_boundary);
    assert_eq!(assistant.tools.len(), 2);
    // The root's own message carries the root session as its thread and
    // stays main-scope.
    assert_eq!(assistant.thread_id.as_deref(), Some("root"));
    assert_eq!(assistant.source, EventSource::Parent);

    let (evidence, store) = evidence_and_rows(&input);
    assert!(matches!(
        evidence.subagents,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
    assert_eq!(observed(&evidence.subagents).spawn_count, 0);
    assert!(
        observed(&evidence.subagents)
            .delegated_models
            .contains("model-child")
    );
    let rows = turn_identities(&store, "root");
    assert_eq!(rows[0].0, "delegated");
    assert_eq!(rows[1].0, "delegated");
    assert_eq!(rows[2].0, "main");

    let retained = serde_json::to_string(&session).expect("serialize normalized session");
    for private in [
        "PRIVATE_REASONING",
        "PRIVATE_PATH",
        "PRIVATE_OUTPUT",
        "PRIVATE_DIFF",
        "PRIVATE_SNAPSHOT",
    ] {
        assert!(!retained.contains(private));
    }
}

#[test]
fn subagent_children_stream_as_delegated_threads() {
    // The task metadata follows OpenCode v1.2.0 packages/opencode/src/tool/task.ts.
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 10);
    insert_session(&connection, "child", Some("root"), None, 20);
    insert_session(&connection, "grandchild", Some("child"), None, 22);
    insert_message(
        &connection,
        "r1",
        "root",
        10,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "c1", "child", 20, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "c2",
        "child",
        30,
        r#"{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":2}}"#,
    );
    insert_message(
        &connection,
        "g1",
        "grandchild",
        32,
        r#"{"role":"assistant","modelID":"model-c","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(
        &connection,
        "r2",
        "root",
        60,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_part(
        &connection,
        "task-child",
        "r1",
        "root",
        11,
        r#"{"type":"tool","tool":"task","callID":"call-child","state":{"status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},"metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},"time":{"start":11,"end":40},"output":"Complete"}}"#,
    );
    insert_part(
        &connection,
        "task-grandchild",
        "c2",
        "child",
        31,
        r#"{"type":"tool","tool":"task","callID":"call-grandchild","state":{"status":"completed","input":{"description":"Inspect tests","prompt":"Inspect tests","subagent_type":"explore"},"metadata":{"sessionId":"grandchild","model":{"providerID":"test","modelID":"model-c"}},"time":{"start":31,"end":40},"output":"Complete"}}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "root");
    let (evidence, store) = evidence_and_rows(&input);

    let rows = turn_identities(&store, "root");
    assert_eq!(
        rows,
        vec![
            ("main".to_owned(), "root".to_owned(), None),
            (
                "delegated".to_owned(),
                "child".to_owned(),
                Some("child".to_owned())
            ),
            (
                "delegated".to_owned(),
                "child".to_owned(),
                Some("child".to_owned())
            ),
            (
                "delegated".to_owned(),
                "grandchild".to_owned(),
                Some("grandchild".to_owned())
            ),
            ("main".to_owned(), "root".to_owned(), None),
        ]
    );

    assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
    assert!(
        matches!(evidence.cache, EvidenceValue::Complete(_)),
        "every row carries its message id, so the thread-identity claim holds: {:?}",
        evidence.cache
    );
    let subagents = observed(&evidence.subagents);
    assert_eq!(subagents.spawn_count, 2);
    assert!(subagents.delegated_models.contains("model-b"));
    assert!(subagents.delegated_models.contains("model-c"));
    assert_eq!(
        subagents.children[0].parent_model.as_deref(),
        Some("model-a")
    );
    assert_eq!(
        subagents.children[1].parent_model.as_deref(),
        Some("model-b")
    );
    assert!(
        subagents
            .children
            .iter()
            .all(|child| child.provenance
                == antiburn_local::analysis::RelationProvenance::TaskToolUse)
    );

    let facts = store.query_turn_facts().expect("turn facts");
    assert!(
        facts.model_transitions.is_empty(),
        "the child's model switch must not read as a root transition"
    );
    assert_eq!(
        facts.longest_idle_gap_ms, 50_000,
        "the child's activity between the root's two messages must not split the root's own gap"
    );
}

#[test]
fn a_fork_is_its_own_root() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(
        &connection,
        "parent",
        None,
        Some("Investigate the outage"),
        10,
    );
    insert_session(
        &connection,
        "fork",
        None,
        Some("Investigate the outage (fork #1)"),
        50,
    );
    insert_message(&connection, "p1", "parent", 10, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "p2",
        "parent",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "p3", "parent", 30, r#"{"role":"user"}"#);
    // Copied prefix: the fork's first three messages carry new ids but keep
    // the parent's own `time_created`, older than the fork session itself.
    insert_message(&connection, "f1", "fork", 10, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "f2",
        "fork",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "f3", "fork", 30, r#"{"role":"user"}"#);
    insert_message(&connection, "f4", "fork", 60, r#"{"role":"user"}"#);
    drop(connection);

    let parent_input = sqlite_input(&path, "parent");
    let mut parent_collector = SessionCollector::new("opencode", "parent");
    reader_for("opencode")
        .visit(&parent_input, &mut parent_collector)
        .expect("stream parent");
    let parent_session = parent_collector.into_session().expect("finished parent");
    assert_eq!(parent_session.events.len(), 3);
    assert!(
        parent_session
            .events
            .iter()
            .all(|event| event.thread_id.as_deref() == Some("parent"))
    );
    let (parent_evidence, _) = evidence_and_rows(&parent_input);
    assert_eq!(observed(&parent_evidence.subagents).spawn_count, 0);

    let fork_input = sqlite_input(&path, "fork");
    let mut fork_collector = SessionCollector::new("opencode", "fork");
    reader_for("opencode")
        .visit(&fork_input, &mut fork_collector)
        .expect("stream fork");
    let fork_session = fork_collector.into_session().expect("finished fork");
    assert_eq!(fork_session.events.len(), 4);
    assert!(
        fork_session
            .events
            .iter()
            .all(|event| event.thread_id.as_deref() == Some("fork")
                && event.source == EventSource::Parent)
    );
    let (fork_evidence, _) = evidence_and_rows(&fork_input);
    assert_eq!(observed(&fork_evidence.subagents).spawn_count, 0);
}

#[test]
fn native_task_requires_child_identity_and_both_models() {
    for missing in [
        "parent_model",
        "task_model",
        "child_model",
        "child_identity",
        "wrong_identity",
        "wrong_model",
        "wrong_parent",
        "pending",
        "subtask",
        "ancestry",
        "metadata",
    ] {
        let (_directory, path) = create_database();
        let connection = Connection::open(&path).expect("database");
        insert_session(&connection, "root", None, None, 10);
        insert_session(&connection, "child", Some("root"), None, 20);
        let mut parent =
            json!({"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}});
        let mut child =
            json!({"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":3}});
        let mut task = json!({"type":"tool","tool":"task","callID":"call-child","state":{
            "status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},
            "metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},
            "time":{"start":11,"end":40},"output":"Complete"
        }});
        match missing {
            "parent_model" => parent["modelID"] = json!(null),
            "task_model" => task["state"]["metadata"]["model"] = json!(null),
            "child_model" => child["modelID"] = json!(null),
            "child_identity" => task["state"]["metadata"]["sessionId"] = json!(null),
            "wrong_identity" => task["state"]["metadata"]["sessionId"] = json!("other-child"),
            "wrong_model" => task["state"]["metadata"]["model"]["modelID"] = json!("other-model"),
            "wrong_parent" => task["state"]["metadata"]["parentSessionId"] = json!("other-parent"),
            "pending" => task["state"]["status"] = json!("pending"),
            "subtask" => {
                parent = json!({"role":"user","model":{"providerID":"test","modelID":"model-a"}});
                task = json!({"type":"subtask","prompt":"Inspect code","description":"Inspect code","agent":"explore","model":{"providerID":"test","modelID":"model-b"}});
            }
            "ancestry" => task = json!({"type":"text","text":"Inspect code"}),
            "metadata" => task["state"]["metadata"] = json!(null),
            _ => unreachable!(),
        }
        insert_message(&connection, "r1", "root", 10, &parent.to_string());
        insert_part(&connection, "task", "r1", "root", 11, &task.to_string());
        insert_message(&connection, "c1", "child", 30, &child.to_string());
        drop(connection);

        let export = [
            json!({"type":"session_meta","payload":{"id":"root"}}),
            json!({"type":"session_member","originSessionID":"child","parentSessionID":"root","payload":{"id":"child"}}),
            json!({"type":"message","sessionID":"root","messageID":"r1","time":{"created":10},"payload":parent}),
            json!({"type":"part","messageID":"r1","payload":task}),
            json!({"type":"message","sessionID":"child","messageID":"c1","time":{"created":30},"payload":child}),
        ].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        for source in [RawSource::Sqlite(path.clone()), RawSource::Jsonl(export)] {
            let input = SessionInput {
                source,
                ..sqlite_input(&path, "root")
            };
            let (evidence, store) = evidence_and_rows(&input);
            assert!(
                matches!(
                    evidence.subagents,
                    EvidenceValue::Partial {
                        reason: CoverageReason::AttributionIncomplete,
                        ..
                    }
                ),
                "{missing}: {:?}",
                evidence.subagents
            );
            let subagents = observed(&evidence.subagents);
            assert_eq!(subagents.spawn_count, 0, "{missing}");
            assert_eq!(subagents.delegated_turns, 1, "{missing}");
            assert!(subagents.children.is_empty(), "{missing}");
            assert_eq!(
                subagents.delegated_models.is_empty(),
                missing == "child_model",
                "{missing}"
            );
            assert_eq!(
                turn_identities(&store, "root"),
                vec![
                    ("main".to_owned(), "root".to_owned(), None),
                    (
                        "delegated".to_owned(),
                        "child".to_owned(),
                        Some("child".to_owned())
                    ),
                ],
                "{missing}"
            );
            let session = reader_for("opencode").normalize(&input).expect("normalize");
            assert_eq!(session.events.len(), 2, "{missing}");
            assert_eq!(session.events[1].usage.input_tokens, 2, "{missing}");
            assert_eq!(session.events[1].usage.output_tokens, 3, "{missing}");
        }
    }
}

#[test]
fn child_model_changes_and_missing_tasks_do_not_pollute_parent_totals() {
    for case in [
        "native",
        "model_switch",
        "wrong_model",
        "no_metadata",
        "no_task",
    ] {
        let (_directory, path) = create_database();
        let connection = Connection::open(&path).expect("database");
        insert_session(&connection, "root", None, None, 10);
        insert_session(&connection, "child", Some("root"), None, 20);
        let mut export = vec![
            json!({"type":"session_meta","time":{"created":10},"payload":{"id":"root"}}),
            json!({"type":"session_member","originSessionID":"child","parentSessionID":"root","payload":{"id":"child"}}),
        ];
        for (index, session_id) in ["root", "child", "child", "root"].iter().enumerate() {
            let id = format!("m{index}");
            let timestamp = 10 + index as i64 * 10;
            let model = if case == "model_switch" && index == 1 {
                "claude-sonnet-4-6"
            } else {
                "claude-opus-4-6"
            };
            let message = json!({"role":"assistant","modelID":model,"providerID":"anthropic",
                "tokens":{"input":if *session_id == "root" { 1 } else { 200_000 },"output":1,
                    "cache":{"read":0,"write":if *session_id == "root" { 0 } else { 200_000 }}}});
            insert_message(
                &connection,
                &id,
                session_id,
                timestamp,
                &message.to_string(),
            );
            export.push(json!({"type":"message","sessionID":session_id,"messageID":id,"time":{"created":timestamp},"payload":message}));
            if index == 0 && case != "no_task" {
                let mut task = json!({"type":"tool","tool":"task","state":{"status":"completed",
                    "metadata":{"sessionId":"child","model":{"modelID":if matches!(case, "model_switch" | "wrong_model") { "claude-sonnet-4-6" } else { model }}}}});
                if case == "no_metadata" {
                    task["state"]["metadata"] = json!(null);
                }
                insert_part(
                    &connection,
                    "task",
                    &id,
                    session_id,
                    timestamp,
                    &task.to_string(),
                );
                export.push(json!({"type":"part","messageID":id,"payload":task}));
            }
        }
        drop(connection);
        for source in [
            RawSource::Sqlite(path.clone()),
            RawSource::Jsonl(
                export
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        ] {
            let input = SessionInput {
                source,
                ..sqlite_input(&path, "root")
            };
            let (evidence, store) = evidence_and_rows(&input);
            let facts = store.query_turn_facts().expect("turn facts");
            let parent_totals: (i64, i64, i64) = store.with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*), SUM(input_tokens), SUM(cache_write_tokens) FROM turn WHERE scope = 'main'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).expect("parent totals")
            });
            assert_eq!(parent_totals, (2, 2, 0), "{case}");
            assert_eq!(facts.repeated_context_cache_write_tokens, 0, "{case}");
            assert_eq!(facts.delegated_turns, 2, "{case}");
            assert!(facts.model_transitions.is_empty(), "{case}");
            let subagents = observed(&evidence.subagents);
            let paired = matches!(case, "native" | "model_switch");
            assert_eq!(subagents.spawn_count, u64::from(paired), "{case}");
            assert_eq!(subagents.children.len(), usize::from(paired), "{case}");
            assert_eq!(
                matches!(evidence.subagents, EvidenceValue::Partial { .. }),
                case != "native",
                "{case}"
            );
            let mut report = EfficiencyReportAccumulator::new();
            report.observe_session(evidence);
            let report = report.finish(ReportContext {
                environment_key: "native".to_owned(),
                window: ReportWindow {
                    start_epoch: 0,
                    end_epoch: 100,
                },
                computed_at_epoch: 100,
                parser_revision: antiburn_local::analysis::PARSER_REVISION,
                analyzer_revision: antiburn_local::analysis::ANALYZER_REVISION,
                evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                coverage: CoverageCounts::default(),
            });
            let counts = report.detectors[DetectorId::OverpoweredSubagents.index()];
            assert_eq!(counts.finding, u64::from(case == "native"), "{case}");
            if case != "native" {
                assert_eq!(counts.clean, 0, "{case}");
                assert_eq!(counts.assessed, 0, "{case}");
            }
        }
    }
}

#[test]
fn a_fork_title_without_a_copied_prefix_is_an_ordinary_root() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(
        &connection,
        "lonely",
        None,
        Some("Refactor payments (fork #2)"),
        10,
    );
    insert_message(&connection, "m1", "lonely", 10, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "m2",
        "lonely",
        20,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "lonely");
    let (evidence, _) = evidence_and_rows(&input);

    assert_eq!(observed(&evidence.subagents).spawn_count, 0);
    assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
}

#[test]
fn a_parent_id_child_with_a_fork_title_degrades_attribution() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, Some("Ship the release"), 10);
    insert_session(
        &connection,
        "child",
        Some("root"),
        Some("Ship the release (fork #1)"),
        20,
    );
    insert_message(
        &connection,
        "r1",
        "root",
        10,
        r#"{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}"#,
    );
    insert_message(&connection, "c1", "child", 20, r#"{"role":"user"}"#);
    insert_message(
        &connection,
        "c2",
        "child",
        30,
        r#"{"role":"assistant","modelID":"model-b","tokens":{"input":1,"output":1}}"#,
    );
    insert_part(
        &connection,
        "task-child",
        "r1",
        "root",
        11,
        r#"{"type":"tool","tool":"task","callID":"call-child","state":{"status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},"metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},"time":{"start":11,"end":40},"output":"Complete"}}"#,
    );
    drop(connection);

    let input = sqlite_input(&path, "root");
    let (evidence, _) = evidence_and_rows(&input);

    match &evidence.subagents {
        EvidenceValue::Partial { reason, .. } => {
            assert_eq!(*reason, CoverageReason::AttributionIncomplete);
        }
        other => panic!("expected partial subagents evidence, got {other:?}"),
    }
    assert_eq!(observed(&evidence.subagents).spawn_count, 0);
    assert!(observed(&evidence.subagents).delegated_models.is_empty());
    let session = reader_for("opencode").normalize(&input).expect("normalize");
    assert!(
        session
            .events
            .iter()
            .all(|event| event.source == EventSource::Parent)
    );
}

#[test]
fn export_stream_marks_a_child_message_as_delegated_with_one_spawn() {
    let jsonl = concat!(
        r#"{"type":"session_meta","sessionID":"root","sessionRole":"root","time":{"created":1000},"payload":{"id":"root","title":"Root session"}}"#,
        "\n",
        r#"{"type":"session_member","rootSessionID":"root","originSessionID":"child","sessionRole":"child","parentSessionID":"root","time":{"created":1500},"payload":{"id":"child","title":"Child session"}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"root","sessionRole":"root","messageID":"m1","time":{"created":1000},"payload":{"role":"assistant","modelID":"model-a","tokens":{"input":1,"output":1}}}"#,
        "\n",
        r#"{"type":"part","messageID":"m1","payload":{"type":"tool","tool":"task","callID":"call-child","state":{"status":"completed","input":{"description":"Inspect code","prompt":"Inspect code","subagent_type":"explore"},"metadata":{"sessionId":"child","model":{"providerID":"test","modelID":"model-b"}},"time":{"start":1500,"end":1700},"output":"Complete"}}}"#,
        "\n",
        r#"{"type":"message","rootSessionID":"root","sessionID":"child","sessionRole":"child","parentSessionID":"root","messageID":"m2","time":{"created":1600},"payload":{"role":"assistant","modelID":"model-b","tokens":{"input":2,"output":1}}}"#,
        "\n",
    );
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "root".to_owned(),
        source: RawSource::Jsonl(jsonl.to_owned()),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };

    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream export");
    let session = collector.into_session().expect("finished session");

    assert_eq!(session.events.len(), 2);
    assert_eq!(session.events[0].thread_id.as_deref(), Some("root"));
    assert_eq!(session.events[0].source, EventSource::Parent);
    assert_eq!(session.events[1].thread_id.as_deref(), Some("child"));
    assert_eq!(session.events[1].source, EventSource::Subagent);

    let (evidence, _) = evidence_and_rows(&input);
    assert_eq!(observed(&evidence.subagents).spawn_count, 1);
}

#[cfg(feature = "test-instrumentation")]
#[test]
fn native_sqlite_does_not_call_discovery_rendering() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 10);
    insert_message(&connection, "message", "root", 20, r#"{"role":"user"}"#);
    drop(connection);
    antiburn_local::discovery::track_provider_db_renders(&path);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    collector.into_session().expect("finished session");

    assert_eq!(
        antiburn_local::discovery::take_tracked_provider_db_renders(&path),
        0
    );
}

#[test]
fn malformed_unknown_and_oversized_rows_report_partial_without_payload() {
    const PRIVATE: &str = "PRIVATE_OVERSIZED_CONTENT";
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 10);
    insert_message(&connection, "malformed", "root", 20, "{not-json");
    insert_message(
        &connection,
        "valid",
        "root",
        30,
        r#"{"role":"assistant","tokens":{"input":1}}"#,
    );
    insert_part(
        &connection,
        "a-unknown",
        "valid",
        "root",
        31,
        &format!(r#"{{"type":"future-part","text":"{PRIVATE}"}}"#),
    );
    let first_part = format!(
        r#"{{"type":"text","text":"{}"}}"#,
        "a".repeat(4 * 1024 * 1024 + 100)
    );
    let second_part = format!(
        r#"{{"type":"text","text":"{PRIVATE}{}"}}"#,
        "b".repeat(4 * 1024 * 1024 + 100)
    );
    insert_part(&connection, "b-large-a", "valid", "root", 32, &first_part);
    insert_part(&connection, "c-large-b", "valid", "root", 33, &second_part);
    let oversized = format!(
        r#"{{"role":"assistant","text":"{}"}}"#,
        "x".repeat(8 * 1024 * 1024)
    );
    insert_message(&connection, "oversized", "root", 40, &oversized);
    drop(connection);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream database");
    let reasons = collector.partial_reasons().clone();
    let session = collector.into_session().expect("finished session");

    assert!(reasons.contains(&PartialReason::MalformedRecord));
    assert!(reasons.contains(&PartialReason::UnrecognizedRecordType));
    assert!(reasons.contains(&PartialReason::Oversized));
    assert_eq!(session.events.len(), 1);
    assert!(!format!("{reasons:?}").contains(PRIVATE));
    assert!(!serde_json::to_string(&session).unwrap().contains(PRIVATE));
}

#[tokio::test]
async fn database_claim_is_checked_inside_the_snapshot() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    insert_session(&connection, "root", None, None, 100);
    insert_message(&connection, "message", "root", 120, r#"{"role":"user"}"#);
    drop(connection);
    let input = sqlite_input(&path, "root");
    let adapter = reader_for("opencode");

    let mut mismatch = SessionCollector::new("opencode", "root");
    let outcome = adapter
        .visit_db_claimed(&input, "sv1:db:0:0", &|| false, &mut mismatch)
        .expect("check mismatch");
    assert!(matches!(outcome, VisitOutcome::SourceChanged(_)));

    let (latest, rows) = Explorers::DISK
        .provider_db_fingerprint(&AgentKind::OpenCode, &path, "root")
        .await
        .expect("database fingerprint");
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let mut matching = SessionCollector::new("opencode", "root");
    let outcome = adapter
        .visit_db_claimed(&input, &fingerprint, &|| false, &mut matching)
        .expect("check matching claim");
    assert_eq!(outcome, VisitOutcome::AcceptedFull);
    assert_eq!(matching.into_session().expect("finished").events.len(), 1);
}

#[tokio::test]
async fn database_fingerprint_includes_uncheckpointed_wal_rows() {
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).expect("database");
    connection
        .execute_batch("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
        .expect("enable WAL");
    insert_session(&connection, "root", None, None, 100);

    let before = Explorers::DISK
        .provider_db_fingerprint(&AgentKind::OpenCode, &path, "root")
        .await
        .expect("initial fingerprint");
    insert_message(
        &connection,
        "wal-message",
        "root",
        120,
        r#"{"role":"user"}"#,
    );

    assert!(path.with_file_name("opencode.db-wal").exists());
    let after = Explorers::DISK
        .provider_db_fingerprint(&AgentKind::OpenCode, &path, "root")
        .await
        .expect("WAL fingerprint");
    assert_ne!(before, after);
}

#[test]
fn exported_messages_stream_without_session_wide_collection() {
    struct CountingSink {
        records: u64,
        finished: bool,
    }

    impl RecordSink for CountingSink {
        fn record(&mut self, record: NormalizedRecord) {
            if matches!(record, NormalizedRecord::MetricsEvent(_)) {
                self.records += 1;
            }
        }

        fn finish(&mut self, _summary: SessionSummary) {
            self.finished = true;
        }
    }

    let mut jsonl = String::new();
    for index in 0..10_000 {
        jsonl.push_str(&format!(
            "{{\"type\":\"message\",\"messageID\":\"m{index}\",\"time\":{{\"created\":{index}}},\"payload\":{{\"role\":\"user\"}}}}\n"
        ));
    }
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "many".to_owned(),
        source: RawSource::Jsonl(jsonl),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };
    let mut sink = CountingSink {
        records: 0,
        finished: false,
    };
    reader_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream synthetic export");

    assert_eq!(sink.records, 10_000);
    assert!(sink.finished);
}

#[test]
fn metrics_and_evidence_publish_from_the_stream() {
    let input = SessionInput { agent: "opencode".to_owned(),
    session_id: "evidence".to_owned(),
    source: RawSource::Jsonl(
        concat!(
            r#"{"type":"message","messageID":"m1","time":{"created":1000},"payload":{"role":"assistant","modelID":"model-a","variant":"high","tokens":{"input":10,"output":2,"reasoning":3,"cache":{"read":4,"write":5}}}}"#,
            "\n",
            r#"{"type":"part","messageID":"m1","payload":{"type":"tool","tool":"read","state":{"input":{"filePath":"PRIVATE_PATH"}}}}"#,
            "\n"
        )
        .to_owned(),
    ),
    fork_parent_session_id: None, source_format: Default::default() };
    let metrics = SessionMetricsAccumulator::new("opencode", "evidence");
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "opencode".to_owned(),
        session_id: "evidence".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::opencode(),
    });
    let store = MemoryTurnRowStore::new("opencode", "evidence");
    let turn_rows = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "evidence",
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, turn_rows);
    let outcome = reader_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream export");
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().expect("published evidence");
    let metrics = sink.metrics().expect("published metrics");

    assert_eq!(metrics.tokens_out, 5);
    assert_eq!(metrics.tokens_in, 15);
    assert_eq!(evidence.capabilities, SourceCapabilities::opencode());
    let models = match &evidence.models {
        EvidenceValue::Complete(models)
        | EvidenceValue::Partial {
            observed: models, ..
        } => models,
        EvidenceValue::Unsupported => panic!("expected model evidence"),
    };
    assert!(models.control_observations.is_empty());
    assert!(!json!(evidence).to_string().contains("PRIVATE_PATH"));
}

#[test]
fn minimal_normalized_schema_reads_parts_and_parent_id_lineage() {
    let directory = TempDir::new().expect("tempdir");
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).expect("database");
    connection
        .execute_batch(include_str!(
            "fixtures/opencode_characterization/minimal_normalized_v1.sql"
        ))
        .expect("fixture schema");
    drop(connection);

    let input = sqlite_input(&path, "root");
    let mut collector = SessionCollector::new("opencode", "root");
    reader_for("opencode")
        .visit(&input, &mut collector)
        .expect("stream minimal normalized schema");
    let session = collector.into_session().expect("finished session");

    assert_eq!(session.events.len(), 2);
    let root = session
        .events
        .iter()
        .find(|event| event.thread_id.as_deref() == Some("root"))
        .expect("root event");
    let child = session
        .events
        .iter()
        .find(|event| event.thread_id.as_deref() == Some("child"))
        .expect("child event");
    assert_eq!(child.source, EventSource::Subagent);
    assert_eq!(root.tools.len(), 0);
    assert!(root.usage.input_tokens > 0);

    let (evidence, _) = evidence_and_rows(&input);
    assert!(matches!(
        evidence.subagents,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
}

#[test]
fn missing_sqlite_session_is_rejected_instead_of_publishing_empty_evidence() {
    let (_directory, path) = create_database();
    let input = sqlite_input(&path, "missing");
    let mut collector = SessionCollector::new("opencode", "missing");

    let error = reader_for("opencode")
        .visit(&input, &mut collector)
        .expect_err("missing session must not look clean");
    assert!(error.to_string().contains("no session missing"));
}

#[test]
fn tool_error_parts_retain_the_error_as_result_content() {
    let mut sink = ContentCapturingSink::default();
    let input = SessionInput {
        agent: "opencode".to_owned(),
        session_id: "tool-error".to_owned(),
        source: RawSource::Jsonl(
            [
                json!({
                    "type": "message",
                    "messageID": "m1",
                    "time": {"created": 1000},
                    "payload": {"role": "assistant", "modelID": "model-a"}
                }),
                json!({
                    "type": "part",
                    "messageID": "m1",
                    "payload": {
                        "type": "tool",
                        "tool": "bash",
                        "state": {"status": "error", "input": {"command": "false"}, "error": "command failed"}
                    }
                }),
            ]
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        ),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };
    reader_for("opencode")
        .visit(&input, &mut sink)
        .expect("stream tool error");

    let parts = &sink.contents[0].parts;
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0].kind,
        antiburn_local::analysis::ContentKind::ToolInput
    );
    assert_eq!(
        parts[1].kind,
        antiburn_local::analysis::ContentKind::ToolResult
    );
    assert_eq!(parts[1].text, "command failed");
}

#[test]
fn native_edit_ranges_and_partial_requests_survive_selected_storage() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    use antiburn_local::analysis::jev_evidence::JevOperationState;
    let (_directory, path) = create_database();
    let connection = Connection::open(&path).unwrap();
    insert_session(&connection, "root", None, None, 1000);
    insert_message(
        &connection,
        "m1",
        "root",
        1000,
        &json!({"role":"assistant"}).to_string(),
    );
    for (index, part) in [
        json!({"type":"tool","tool":"edit","callID":"edit","state":{"status":"completed","input":{"filePath":"src/é.rs","cwd":"/repo","oldString":"old","newString":"PRIVATE_EDIT_BODY"}}}),
        json!({"type":"tool","tool":"bash","callID":"partial","state":{"status":"running","input":{"command":"x".repeat(antiburn_local::analysis::MAX_CONTENT_PART_BYTES + 1)}}}),
        json!({"type":"tool","tool":"apply_patch","callID":"patch","state":{"status":"future-state","input":{"patchText":"*** Begin Patch\n*** Update File: src/a.rs\n+text\n*** End Patch"}}}),
    ].iter().enumerate() {
        insert_part(&connection, &format!("p{index}"), "m1", "root", 1001 + index as i64, &part.to_string());
    }
    drop(connection);
    let (_, store) = evidence_and_rows(&sqlite_input(&path, "root"));
    store.with_connection(|connection| {
        for field in [
            JevInputField::FileEditPath,
            JevInputField::FileEditContent,
            JevInputField::BashCommandInput,
        ] {
            let selection = JevInputSelection::from_fields(&[field]);
            let published = antiburn_local::analysis::query_turn_content_offset_selected(
                connection,
                &antiburn_local::analysis::TurnSessionKey {
                    environment_key: "native",
                    agent: "opencode",
                    session_id: "root",
                },
                &antiburn_local::analysis::FenceScope::single(1),
                None,
                &Default::default(),
                0,
                selection,
            )
            .unwrap();
            if field == JevInputField::BashCommandInput {
                let partial = &published.parts[0].part;
                assert!(partial.truncated);
                assert_eq!(partial.metadata.state, JevOperationState::Running);
                assert!(partial.metadata.bindings.is_empty());
                assert!(partial.normalized_fields.as_ref().unwrap().malformed);
                continue;
            }
            let edit = published
                .parts
                .iter()
                .find(|part| part.part.tool_call_id.as_deref() == Some("edit"))
                .unwrap();
            assert_eq!(edit.part.metadata.state, JevOperationState::Completed);
            assert!(
                edit.part
                    .metadata
                    .bindings
                    .iter()
                    .all(|binding| binding.field == field)
            );
            if field == JevInputField::FileEditPath {
                assert_eq!(
                    edit.part
                        .metadata
                        .bindings
                        .iter()
                        .find(|binding| binding.pointer.ends_with("/filePath"))
                        .unwrap()
                        .end,
                    "src/é.rs".len()
                );
                let facts =
                    antiburn_local::analysis::jev_evidence::JevRecordedPathFacts::from_selected(
                        field,
                        &edit.part.normalized_fields.as_ref().unwrap().values[&field],
                        None,
                    );
                assert_eq!(facts.cwd.as_deref(), Some("/repo"));
                assert_eq!(facts.paths, ["src/é.rs"]);
                assert_eq!(facts.resolve("src/é.rs"), None);
                assert!(!format!("{:?}", published).contains("PRIVATE_EDIT_BODY"));
            } else {
                assert_eq!(edit.part.metadata.bindings.len(), 2);
            }
            let patch = published
                .parts
                .iter()
                .find(|part| part.part.tool_call_id.as_deref() == Some("patch"))
                .unwrap();
            assert_eq!(patch.part.metadata.state, JevOperationState::Unknown);
            assert_eq!(patch.part.metadata.bindings.len(), 1);
            assert_eq!(patch.part.metadata.bindings[0].field, field);
            assert!(
                patch.part.metadata.bindings[0]
                    .pointer
                    .ends_with("/patchText")
            );
            assert!(!patch.part.normalized_fields.as_ref().unwrap().malformed);
            let selected = &patch.part.normalized_fields.as_ref().unwrap().values[&field];
            if field == JevInputField::FileEditPath {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(selected).unwrap()["paths"],
                    json!(["src/a.rs"])
                );
                assert!(!selected.contains("+text"));
            } else {
                assert!(selected.contains("+text"));
            }
        }
    });
}
