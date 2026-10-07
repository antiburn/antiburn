use super::*;
use antiburn_local::analysis::jev::{JevFieldCapability, JevInputField};
use antiburn_local::analysis::jev_evidence::{
    JevPlanContentStatus, JevPlanStatus, JevUserAnswerOrigin, JevUserAnswerStatus, field_capability,
};
use antiburn_local::analysis::{ContentKind, ContentPart, SourceFormat};

fn capture(source: &str) -> Vec<ContentPart> {
    let input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        source: RawSource::Jsonl(source.into()),
        fork_parent_session_id: None,
        source_format: SourceFormat::ClaudeJsonl,
    };
    let mut sink = ClaudeContentSink::default();
    reader_for("claude")
        .visit(&input, &mut sink)
        .expect("Claude scope fixture must stream");
    sink.contents.into_iter().flat_map(|c| c.parts).collect()
}

fn records(values: &[Value]) -> String {
    let mut parent = Value::Null;
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let mut value = value.clone();
            if value.get("uuid").is_none() {
                value["uuid"] = json!(format!("00000000-0000-0000-0000-{:012x}", index + 1));
            }
            if value.get("parentUuid").is_none() && value.get("logicalParentUuid").is_none() {
                value["parentUuid"] = parent.clone();
            }
            parent = value["uuid"].clone();
            value.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn question_input() -> Value {
    json!({"questions":[{"question":"Scope?", "header":"Scope", "multiSelect":false,
        "options":[{"label":"API", "description":"Do not change billing."}, {"label":"Both", "description":"Also change billing."}]}]})
}

fn call(name: &str, input: Value) -> Value {
    json!({"type":"assistant", "message":{"role":"assistant", "content":[{
        "type":"tool_use", "id":"call-1", "name":name, "input":input}]}})
}

fn result(payload: Value) -> Value {
    json!({"type":"user", "message":{"role":"user", "content":[{
        "type":"tool_result", "tool_use_id":"call-1", "content":"approved"}]}, "toolUseResult":payload})
}

#[test]
fn native_scope_records_keep_exact_answers_and_distinct_plan_versions_without_authority() {
    let source = include_str!("../fixtures/claude_characterization/scope_records.jsonl");
    let parts = capture(source);
    let answers: Vec<_> = parts
        .iter()
        .flat_map(|p| &p.metadata.user_answers)
        .collect();
    assert_eq!(answers.len(), 4);
    assert_eq!(
        answers[0].options[0].description.as_deref(),
        Some("Keep billing unchanged.\n  Do not add retries.")
    );
    assert_eq!(answers[0].status, JevUserAnswerStatus::Pending);
    assert_eq!(answers[2].status, JevUserAnswerStatus::Submitted);
    assert_eq!(answers[2].selections[0].option_index, Some(0));
    assert_eq!(answers[3].multi_select, Some(true));
    assert_eq!(
        answers[3].free_text.as_deref(),
        Some("Cache, Logs, keep retries at 2\nActually, do not change billing.")
    );
    assert_eq!(answers[3].selections.len(), 1);
    assert_eq!(answers[3].selections[0].option_index, None);
    assert!(
        answers
            .iter()
            .all(|a| a.origin == JevUserAnswerOrigin::UnknownOrigin
                && !a.is_authoritative_user_response())
    );
    let plans: Vec<_> = parts
        .iter()
        .flat_map(|p| &p.metadata.plan_references)
        .collect();
    assert_eq!(plans.len(), 5);
    assert_eq!(plans[0].text.as_deref(), Some("# Scope\n- API only\n"));
    assert_eq!(plans[1].text, plans[2].text);
    assert_eq!(plans[1].content_digest, plans[2].content_digest);
    assert_ne!(plans[3].content_digest, plans[4].content_digest);
    assert_eq!(plans[3].status, JevPlanStatus::Proposed);
    assert!(plans.iter().all(|p| p.status != JevPlanStatus::Approved
        && p.approved_content_digest.is_none()
        && p.approved_revision.is_none()));
    assert!(
        plans
            .iter()
            .all(|p| p.content_status == JevPlanContentStatus::Recorded)
    );
    assert!(parts.iter().any(|p| p.kind == ContentKind::UserText
        && p.text == "No, keep the prior retry limit. Implement API only."));
    for field in [JevInputField::UserAnswer, JevInputField::PlanReference] {
        assert_eq!(
            field_capability(SourceFormat::ClaudeJsonl, field),
            JevFieldCapability::Conditional
        );
        assert_eq!(
            field_capability(SourceFormat::CodexRolloutJsonl, field),
            JevFieldCapability::Conditional
        );
    }
}

#[test]
fn decoder_answer_only_shape_and_public_rejection_free_text_remain_separate() {
    let parts = capture(&records(&[
        call("AskUserQuestion", json!({})),
        result(json!({"answers":{"Pick one":"Enable via API"}})),
    ]));
    let answer = &parts
        .iter()
        .find(|p| !p.metadata.user_answers.is_empty())
        .unwrap()
        .metadata
        .user_answers[0];
    assert_eq!(answer.prompt, "Pick one");
    assert!(answer.options.is_empty());
    assert_eq!(answer.status, JevUserAnswerStatus::Submitted);
    assert!(!answer.is_authoritative_user_response());
    let mut rejected = result(json!("User rejected tool use"));
    rejected["message"]["content"][0]["is_error"] = json!(true);
    let reply =
        json!({"type":"user", "message":{"role":"user", "content":"Use my custom API design."}});
    let parts = capture(&records(&[
        call("AskUserQuestion", question_input()),
        rejected,
        reply,
    ]));
    assert!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .all(|a| a.selections.is_empty() && !a.is_authoritative_user_response())
    );
    assert!(
        parts
            .iter()
            .any(|p| p.kind == ContentKind::UserText && p.text == "Use my custom API design.")
    );
}

#[test]
fn malformed_missing_interrupted_synthetic_and_conflicting_answers_never_gain_authority() {
    for payload in [
        json!({}),
        json!({"answers":null}),
        json!({"answers":{"Scope?":null}}),
        json!({"answers":{"Scope?":[]}}),
        json!({"answers":{"Scope?":""}}),
    ] {
        let parts = capture(&records(&[
            call("AskUserQuestion", question_input()),
            result(payload),
        ]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted)
        );
    }
    for flag in [
        "interruptedByShutdown",
        "isMeta",
        "isSynthetic",
        "isCompactSummary",
    ] {
        let mut output = result(json!({"answers":{"Scope?":"API"}}));
        output[flag] = json!(true);
        let parts = capture(&records(&[
            call("AskUserQuestion", question_input()),
            output,
        ]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted)
        );
    }
    let mut changed = question_input();
    changed["questions"][0]["options"][0]["description"] = json!("Also change billing.");
    let parts = capture(&records(&[
        call("AskUserQuestion", question_input()),
        result(json!({"questions":changed["questions"], "answers":{"Scope?":"API"}})),
    ]));
    let answers: Vec<_> = parts
        .iter()
        .flat_map(|p| &p.metadata.user_answers)
        .collect();
    assert!(
        answers
            .iter()
            .any(|a| a.options[0].description.as_deref() == Some("Also change billing."))
    );
    assert!(
        answers
            .iter()
            .all(|a| a.status != JevUserAnswerStatus::Submitted)
    );
}

#[test]
fn updated_sdk_input_and_text_approval_are_not_cli_answers() {
    let ask = call("AskUserQuestion", question_input());
    for payload in [
        json!({"updatedInput":{"questions":question_input()["questions"], "answers":{"Scope?":"API"}}}),
        json!({"response":"API"}),
        json!("User has answered your questions: API"),
    ] {
        let parts = capture(&records(&[ask.clone(), result(payload)]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted && a.selections.is_empty())
        );
    }
    let payload = json!({"questions":question_input()["questions"], "answers":{"Scope?":"API"}});
    let parts = capture(&records(&[ask.clone(), result(payload.clone())]));
    assert!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .any(|a| a.status == JevUserAnswerStatus::Submitted)
    );
    for flag in ["interrupted", "truncated"] {
        let mut payload = payload.clone();
        payload[flag] = json!(true);
        let parts = capture(&records(&[ask.clone(), result(payload)]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted)
        );
    }
}

#[test]
fn exact_call_identity_rejects_other_tools_duplicates_branches_and_ambiguous_results() {
    for name in ["Bash", "mcp__test__AskUserQuestion", "Unknown"] {
        let parts = capture(&records(&[
            call(name, question_input()),
            result(json!({"answers":{"Scope?":"API"}})),
        ]));
        assert!(parts.iter().all(|p| p.metadata.user_answers.is_empty()));
    }
    let ask = call("AskUserQuestion", question_input());
    for second in [
        ask.clone(),
        call("Bash", json!({})),
        call("AskUserQuestion", json!({"questions":[]})),
    ] {
        let parts = capture(&records(&[
            ask.clone(),
            second,
            result(json!({"answers":{"Scope?":"API"}})),
        ]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted)
        );
    }
    for field in ["sessionId", "agentId", "sourceToolUseID"] {
        let mut output = result(json!({"answers":{"Scope?":"API"}}));
        output[field] = json!("other");
        let parts = capture(&records(&[ask.clone(), output]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted)
        );
    }
    let mut output = result(json!({"answers":{"Scope?":"API"}}));
    output["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"tool_result", "tool_use_id":"other", "content":"other"}));
    let parts = capture(&records(&[ask, output]));
    assert!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .all(|a| a.status != JevUserAnswerStatus::Submitted)
    );
}

#[test]
fn conflicting_native_roles_and_duplicate_prompts_do_not_create_submitted_answers() {
    let mut ask = call("AskUserQuestion", question_input());
    ask["message"]["role"] = json!("user");
    let parts = capture(&records(&[
        ask,
        result(json!({"answers":{"Scope?":"API"}})),
    ]));
    assert!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .all(|a| a.status != JevUserAnswerStatus::Submitted)
    );
    let mut questions = question_input();
    let duplicate = questions["questions"][0].clone();
    questions["questions"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    let parts = capture(&records(&[
        call("AskUserQuestion", questions),
        result(json!({"answers":{"Scope?":"API"}})),
    ]));
    assert!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .all(|a| a.status != JevUserAnswerStatus::Submitted
                && a.selections.iter().all(|s| s.option_index.is_none()))
    );
}

#[test]
fn plan_permissions_errors_and_mutable_references_never_bind_approved_versions() {
    for payload in [
        json!({}),
        json!({"filePath":"/synthetic/.claude/plans/task.md"}),
        json!("User rejected tool use"),
        json!({"plan":"# Recorded", "filePath":"/synthetic/.claude/plans/task.md"}),
    ] {
        let parts = capture(&records(&[
            call(
                "ExitPlanMode",
                json!({"planFilePath":"/synthetic/.claude/plans/task.md"}),
            ),
            result(payload),
        ]));
        let plans: Vec<_> = parts
            .iter()
            .flat_map(|p| &p.metadata.plan_references)
            .collect();
        assert_eq!(plans.len(), 2);
        assert!(
            plans
                .iter()
                .all(|p| p.status != JevPlanStatus::Approved && p.approved_revision.is_none())
        );
        assert_eq!(plans[0].content_status, JevPlanContentStatus::Unresolved);
    }
    let parts = capture(&records(&[
        json!({"type":"user", "planContent":"# Cached version\nDo not edit billing.", "message":{"role":"user", "content":"Implement the following plan:\n\n# Cached version\nDo not edit billing."}}),
    ]));
    let plan = &parts
        .iter()
        .find(|p| !p.metadata.plan_references.is_empty())
        .unwrap()
        .metadata
        .plan_references[0];
    assert_eq!(plan.origin, JevUserAnswerOrigin::Synthetic);
    assert_eq!(plan.status, JevPlanStatus::Unknown);
}

#[test]
fn incomplete_and_mismatched_file_results_do_not_become_full_plan_versions() {
    for file in [
        json!({"filePath":"/synthetic/.claude/plans/task.md", "content":"partial", "startLine":2,"numLines":2,"totalLines":2}),
        json!({"filePath":"/synthetic/.claude/plans/task.md", "content":"partial", "startLine":1,"numLines":2,"totalLines":3}),
        json!({"filePath":"/other.md", "content":"wrong", "startLine":1,"numLines":2,"totalLines":2}),
        json!({"filePath":"/synthetic/.claude/plans/task.md", "content":"unknown extent"}),
    ] {
        let parts = capture(&records(&[
            call(
                "Read",
                json!({"file_path":"/synthetic/.claude/plans/task.md"}),
            ),
            result(json!({"type":"text", "file":file})),
        ]));
        let plan = &parts
            .iter()
            .find(|p| !p.metadata.plan_references.is_empty())
            .unwrap()
            .metadata
            .plan_references[0];
        assert_eq!(plan.content_status, JevPlanContentStatus::Unresolved);
        assert!(plan.text.is_none());
    }
    let parts = capture(&records(&[
        call(
            "Write",
            json!({"file_path":"/synthetic/.claude/plans/task.md", "content":"requested"}),
        ),
        result(
            json!({"type":"update", "filePath":"/synthetic/.claude/plans/task.md", "content":"different"}),
        ),
    ]));
    let plans: Vec<_> = parts
        .iter()
        .flat_map(|p| &p.metadata.plan_references)
        .collect();
    assert_eq!(plans[0].status, JevPlanStatus::Proposed);
    assert_eq!(plans[0].text.as_deref(), Some("requested"));
    assert!(plans[1].text.is_none());
}

#[test]
fn malformed_record_breaks_pending_joins() {
    let source = format!(
        "{}\n{{malformed\n{}",
        call("AskUserQuestion", question_input()),
        result(json!({"answers":{"Scope?":"API"}}))
    );
    let parts = capture(&source);
    assert!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .all(|a| a.status != JevUserAnswerStatus::Submitted)
    );
}

#[test]
fn stored_scope_fields_are_explicit_and_do_not_widen_ignored_instructions() {
    use antiburn_local::analysis::jev::JevInputSelection;
    use antiburn_local::analysis::jev_evidence::{prepare_session_content, select_session_content};
    use antiburn_local::analysis::{
        FenceScope, TurnSessionKey, query_turn_content_offset_selected,
    };
    let input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-store".into(),
        source: RawSource::Jsonl(
            include_str!("../fixtures/claude_characterization/scope_records.jsonl").into(),
        ),
        fork_parent_session_id: None,
        source_format: SourceFormat::ClaudeJsonl,
    };
    let store = MemoryTurnRowStore::new("claude", "scope-store");
    let mut sink = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "scope-store",
        None,
    );
    reader_for("claude").visit(&input, &mut sink).unwrap();
    store.with_connection(|connection| {
        let query = |selection| {
            query_turn_content_offset_selected(
                connection,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "claude",
                    session_id: "scope-store",
                },
                &FenceScope::single(1),
                None,
                &Default::default(),
                0,
                selection,
            )
            .unwrap()
        };
        let fields = JevInputSelection::from_fields(&[
            JevInputField::UserAnswer,
            JevInputField::PlanReference,
        ]);
        let content = query(fields);
        assert_eq!(
            content
                .parts
                .iter()
                .flat_map(|p| &p.part.metadata.user_answers)
                .count(),
            4
        );
        assert_eq!(
            content
                .parts
                .iter()
                .flat_map(|p| &p.part.metadata.plan_references)
                .count(),
            5
        );
        let selected = select_session_content(
            &prepare_session_content(
                "scope-store",
                SourceFormat::ClaudeJsonl,
                content,
                Vec::new(),
            ),
            fields,
        );
        assert!(
            selected
                .actions
                .iter()
                .any(|a| !a.metadata.user_answers.is_empty())
        );
        assert!(
            selected
                .actions
                .iter()
                .any(|a| !a.metadata.plan_references.is_empty())
        );
        for selection in [
            JevInputSelection::ALL,
            antiburn_local::checks::ignored_instructions::INPUT_SELECTION,
        ] {
            let content = query(selection);
            assert!(
                content
                    .parts
                    .iter()
                    .all(|p| p.part.metadata.user_answers.is_empty()
                        && p.part.metadata.plan_references.is_empty())
            );
        }
    });
}

#[test]
fn recorded_native_ids_and_branch_links_bind_but_sibling_and_sidechain_results_do_not() {
    let mut ask = call("AskUserQuestion", question_input());
    ask["uuid"] = json!("00000000-0000-0000-0000-000000000001");
    let mut output = result(json!({"answers":{"Scope?":"API"}}));
    output["uuid"] = json!("00000000-0000-0000-0000-000000000002");
    output["parentUuid"] = ask["uuid"].clone();
    let parts = capture(&records(&[ask.clone(), output.clone()]));
    let answers: Vec<_> = parts
        .iter()
        .flat_map(|p| &p.metadata.user_answers)
        .collect();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[1].status, JevUserAnswerStatus::Submitted);
    assert_eq!(
        answers[1].source.native_record_id.as_deref(),
        output["uuid"].as_str()
    );
    let bindings = &answers[1].source.bindings;
    assert!(
        bindings
            .iter()
            .any(|binding| binding.pointer == "/toolUseResult/answers/Scope?"
                && binding.native_record_id.as_deref() == output["uuid"].as_str())
    );
    assert!(bindings.iter().any(|binding| binding.pointer
        == "/message/content/0/input/questions/0/question"
        && binding.native_record_id.as_deref() == ask["uuid"].as_str()));
    assert_eq!(answers[1].header, answers[0].header);
    for binding in bindings {
        let native = if binding.native_record_id.as_deref() == ask["uuid"].as_str() {
            &ask
        } else {
            &output
        };
        let text = native.pointer(&binding.pointer).unwrap().as_str().unwrap();
        assert_eq!(text.get(binding.start..binding.end), Some(text));
    }
    for (field, value) in [("parentUuid", Value::Null), ("isSidechain", json!(true))] {
        let mut other = output.clone();
        other[field] = value;
        let parts = capture(&records(&[ask.clone(), other]));
        assert!(
            parts
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .all(|a| a.status != JevUserAnswerStatus::Submitted)
        );
    }
}

fn resumed_parts(values: &[Value], boundaries: &[usize]) -> Vec<ContentPart> {
    use antiburn_local::analysis::{
        EvidenceSnapshot, RESUME_SNAPSHOT_REVISION, ResumePoint, SourceClaim, StreamSnapshot,
    };
    use antiburn_local::discovery::source_version::head_hash_of;
    use antiburn_local::discovery::{FingerprintInputs, SourceStat};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scope.jsonl");
    let full = format!("{}\n", records(values));
    fs::write(&path, "").unwrap();
    let claim = || {
        let file = fs::File::open(&path).unwrap();
        SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
            stat: SourceStat::from_open_std_file(&file).unwrap(),
            head_hash: Some(head_hash_of(&fs::read(&path).unwrap())),
        })
    };
    let mut input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        source: RawSource::File(path.clone()),
        fork_parent_session_id: None,
        source_format: SourceFormat::ClaudeJsonl,
    };
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    });
    let mut snapshot = StreamSnapshot {
        revision: RESUME_SNAPSHOT_REVISION,
        resume: ResumePoint {
            offset: 0,
            tail_hash: head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: reader_for("claude").empty_resume_state().unwrap(),
        metrics: SessionMetricsAccumulator::new("claude", "scope-test"),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: Default::default(),
        },
        next_turn_index: 0,
    };
    let mut sink = ClaudeContentSink::default();
    assert_eq!(boundaries.last(), Some(&values.len()));
    for &count in boundaries {
        fs::write(&path, format!("{}\n", records(&values[..count]))).unwrap();
        let visit = reader_for("claude")
            .visit_claimed_resumed(&input, &claim(), &snapshot, &|| false, &mut sink)
            .unwrap();
        let resume = visit.resume.unwrap();
        snapshot.resume = resume.point;
        snapshot.adapter = resume.adapter;
    }
    input.source = RawSource::Jsonl(full);
    let mut full_sink = ClaudeContentSink::default();
    reader_for("claude").visit(&input, &mut full_sink).unwrap();
    assert_eq!(sink.contents, full_sink.contents);
    sink.contents.into_iter().flat_map(|c| c.parts).collect()
}

#[test]
fn scope_call_state_survives_claimed_resume_and_matches_full_reader() {
    let parts = resumed_parts(
        &[
            call("AskUserQuestion", question_input()),
            result(json!({"answers":{"Scope?":"API"}})),
        ],
        &[1, 2],
    );
    assert_eq!(
        parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .filter(|a| a.status == JevUserAnswerStatus::Submitted)
            .count(),
        1
    );
}

fn scope_scenarios() -> Vec<(&'static str, Value, Value)> {
    let path = "/synthetic/.claude/plans/task.md";
    vec![
        (
            "AskUserQuestion",
            question_input(),
            json!({"answers":{"Scope?":"API"}}),
        ),
        (
            "ExitPlanMode",
            json!({"planFilePath":path, "plan":"# Recorded"}),
            json!({"filePath":path, "plan":"# Recorded", "isAgent":false}),
        ),
        (
            "Read",
            json!({"file_path":path}),
            json!({"type":"text", "file":{"filePath":path, "content":"# Recorded", "startLine":1, "numLines":1, "totalLines":1}}),
        ),
        (
            "Write",
            json!({"file_path":path, "content":"# Recorded"}),
            json!({"type":"update", "filePath":path, "content":"# Recorded"}),
        ),
    ]
}

fn node(mut record: Value, id: &str, parent: Value) -> Value {
    record["uuid"] = json!(id);
    record["parentUuid"] = parent;
    record
}

const ROOT_RECORD: &str = "00000000-0000-0000-0000-000000000001";
const CALL_RECORD: &str = "00000000-0000-0000-0000-000000000002";
const SIBLING_RESULT: &str = "00000000-0000-0000-0000-000000000003";
const SIBLING_DESCENDANT_RESULT: &str = "00000000-0000-0000-0000-000000000004";
const BRIDGE_RECORD: &str = "00000000-0000-0000-0000-000000000005";
const DESCENDANT_RESULT: &str = "00000000-0000-0000-0000-000000000006";

fn branched_records(name: &str, input: Value, payload: Value) -> Vec<Value> {
    vec![
        node(
            json!({"type":"user", "message":{"role":"user", "content":"Review API only."}}),
            ROOT_RECORD,
            Value::Null,
        ),
        node(call(name, input), CALL_RECORD, json!(ROOT_RECORD)),
        node(result(payload.clone()), SIBLING_RESULT, json!(ROOT_RECORD)),
        node(
            result(payload.clone()),
            SIBLING_DESCENDANT_RESULT,
            json!(SIBLING_RESULT),
        ),
        node(
            json!({"type":"assistant", "message":{"role":"assistant", "content":"Recorded continuation."}}),
            BRIDGE_RECORD,
            json!(CALL_RECORD),
        ),
        node(result(payload), DESCENDANT_RESULT, json!(BRIDGE_RECORD)),
    ]
}

fn assert_only_descendant_result(parts: &[ContentPart], name: &str) {
    for part in parts {
        for source in part
            .metadata
            .user_answers
            .iter()
            .map(|a| &a.source)
            .chain(part.metadata.plan_references.iter().map(|p| &p.source))
        {
            assert!(!matches!(
                source.native_record_id.as_deref(),
                Some(SIBLING_RESULT | SIBLING_DESCENDANT_RESULT)
            ));
        }
    }
    if name == "AskUserQuestion" {
        let submitted: Vec<_> = parts
            .iter()
            .flat_map(|p| &p.metadata.user_answers)
            .filter(|a| a.status == JevUserAnswerStatus::Submitted)
            .collect();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].source.native_record_id.as_deref(),
            Some(DESCENDANT_RESULT)
        );
    } else {
        let results: Vec<_> = parts
            .iter()
            .flat_map(|p| &p.metadata.plan_references)
            .filter(|p| {
                p.source.role == antiburn_local::analysis::jev_evidence::JevScopeEvidenceRole::Tool
            })
            .collect();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].source.native_record_id.as_deref(),
            Some(DESCENDANT_RESULT)
        );
        assert_eq!(results[0].text.as_deref(), Some("# Recorded"));
    }
}

#[test]
fn same_root_sibling_results_leave_all_scope_call_metadata_for_true_descendants() {
    for (name, input, payload) in scope_scenarios() {
        let values = branched_records(name, input, payload);
        let session = normalize_source(&SessionInput {
            agent: "claude".into(),
            session_id: "scope-test".into(),
            source: RawSource::Jsonl(records(&values)),
            fork_parent_session_id: None,
            source_format: SourceFormat::ClaudeJsonl,
        })
        .unwrap();
        assert!(
            session
                .events
                .iter()
                .all(|event| event.thread_id.as_deref() == Some(ROOT_RECORD))
        );
        let siblings_only = capture(&records(&values[..4]));
        assert!(
            !siblings_only
                .iter()
                .flat_map(|p| &p.metadata.user_answers)
                .any(|a| a.status == JevUserAnswerStatus::Submitted)
        );
        assert!(
            !siblings_only
                .iter()
                .flat_map(|p| &p.metadata.plan_references)
                .any(|p| p.source.role
                    == antiburn_local::analysis::jev_evidence::JevScopeEvidenceRole::Tool)
        );
        assert_only_descendant_result(&capture(&records(&values)), name);
    }
}

#[test]
fn sibling_rejection_and_true_descendant_binding_survive_each_claimed_resume_boundary() {
    for (name, input, payload) in scope_scenarios() {
        let values = branched_records(name, input, payload);
        assert_only_descendant_result(&resumed_parts(&values, &[2, 4, 5, 6]), name);
    }
}

#[test]
fn missing_unresolved_and_cyclic_ancestry_deny_scope_results() {
    for (name, input, payload) in scope_scenarios() {
        for shape in [
            "missing-call-id",
            "missing-result-id",
            "missing-parent",
            "unresolved-root",
            "cyclic-root",
        ] {
            let mut values = branched_records(name, input.clone(), payload.clone());
            match shape {
                "missing-call-id" => values[1]["uuid"] = Value::Null,
                "missing-result-id" => values[5]["uuid"] = Value::Null,
                "missing-parent" => {
                    values[5].as_object_mut().unwrap().remove("parentUuid");
                }
                "unresolved-root" => values[0]["parentUuid"] = json!("missing"),
                "cyclic-root" => values[0]["parentUuid"] = json!(DESCENDANT_RESULT),
                _ => unreachable!(),
            }
            // Serialize exact missing fields without the normal fixture chain helper.
            let source = values
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            let parts = capture(&source);
            assert!(
                !parts
                    .iter()
                    .flat_map(|p| &p.metadata.user_answers)
                    .any(|a| a.status == JevUserAnswerStatus::Submitted),
                "{name}: {shape}"
            );
            assert!(
                !parts
                    .iter()
                    .flat_map(|p| &p.metadata.plan_references)
                    .any(|p| p.source.role
                        == antiburn_local::analysis::jev_evidence::JevScopeEvidenceRole::Tool),
                "{name}: {shape}"
            );
        }
    }
}

fn native_values() -> Vec<Value> {
    include_str!("../fixtures/claude_characterization/retained_native_results.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn retained_native_root_binds_human_text_reads_tests_and_failed_skills() {
    use antiburn_local::analysis::ContentAuthority;
    use antiburn_local::analysis::jev_evidence::{JevOperationState, JevReadStatus, JevReadUnit};
    let values = native_values();
    let parts = capture(&records(&values));
    let human: Vec<_> = parts
        .iter()
        .filter(|part| part.metadata.user_text_history.is_some())
        .collect();
    assert_eq!(human.len(), 2);
    assert!(
        human
            .iter()
            .all(|part| part.authority == ContentAuthority::User)
    );
    assert_eq!(human[1].text, "Stop. Do not make further edits.");
    assert!(
        parts
            .iter()
            .filter(|part| part.kind == ContentKind::UserText
                && part.authority != ContentAuthority::User)
            .all(|part| part.metadata.user_text_history.is_none())
    );
    let read = parts
        .iter()
        .find(|part| part.metadata.read_result.is_some())
        .unwrap();
    let result = read.metadata.read_result.as_ref().unwrap();
    assert_eq!(result.status, JevReadStatus::Success);
    let extent = result.returned_extent.as_ref().unwrap();
    assert_eq!(
        (
            extent.unit,
            extent.offset,
            extent.limit,
            extent.end_inclusive
        ),
        (JevReadUnit::Lines, Some(7), Some(3), Some(9))
    );
    assert!(!result.truncated);
    assert!(result.recorded_file_version.is_none());
    assert_eq!(result.recorded_output_bytes, read.text.len() as u64);
    let test = parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolResult && part.tool_call_id.as_deref() == Some("test-1")
        })
        .unwrap();
    assert_eq!(test.metadata.state, JevOperationState::Completed);
    assert_eq!(test.text, "test result: ok. 2 passed; 0 failed");
    let skill = parts
        .iter()
        .find(|part| {
            part.kind == ContentKind::ToolResult && part.tool_call_id.as_deref() == Some("skill-1")
        })
        .unwrap();
    assert_eq!(skill.metadata.state, JevOperationState::Error);
    assert_eq!(skill.authority, ContentAuthority::Tool);
    for part in &parts {
        for binding in &part.metadata.bindings {
            let record = values
                .iter()
                .find(|record| record["uuid"].as_str() == binding.native_record_id.as_deref())
                .unwrap();
            let text = record.pointer(&binding.pointer).unwrap().as_str().unwrap();
            assert!(text.get(binding.start..binding.end).is_some());
        }
    }
    resumed_parts(&values, &[2, 3, 6, 7, 10]);
}

#[test]
fn native_read_extent_uses_observed_lines_and_rejects_mismatched_metadata() {
    for (pointer, value) in [
        ("/toolUseResult/file/filePath", json!("/other")),
        ("/toolUseResult/file/startLine", json!(6)),
        ("/toolUseResult/file/numLines", json!(4)),
        ("/toolUseResult/file/totalLines", json!(8)),
        (
            "/toolUseResult/file/content",
            json!("different\neight\nnine"),
        ),
    ] {
        let mut values = native_values();
        *values[2].pointer_mut(pointer).unwrap() = value;
        let parts = capture(&records(&values[..3]));
        let read = parts
            .iter()
            .find_map(|part| part.metadata.read_result.as_ref())
            .unwrap();
        assert!(read.returned_extent.is_none(), "{pointer}");
    }
    let mut values = native_values();
    values[1]["message"]["content"][0]["input"]["limit"] = json!(100);
    let parts = capture(&records(&values[..3]));
    let extent = parts
        .iter()
        .find_map(|part| part.metadata.read_result.as_ref())
        .unwrap()
        .returned_extent
        .as_ref()
        .unwrap();
    assert_eq!(extent.limit, Some(3));
}

#[test]
fn native_results_keep_failure_interruption_clipping_and_unknown_status_distinct() {
    use antiburn_local::analysis::jev_evidence::{JevOperationState, JevReadStatus};
    for control in [
        "error",
        "missing-status",
        "interrupted",
        "persisted",
        "partial",
        "synthetic",
    ] {
        let mut values = native_values();
        match control {
            "error" => values[2]["message"]["content"][0]["is_error"] = json!(true),
            "missing-status" => {
                values[2]["message"]["content"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("is_error");
                values[2].as_object_mut().unwrap().remove("toolUseResult");
            }
            "interrupted" => values[2]["toolUseResult"]["interrupted"] = json!(true),
            "persisted" => {
                values[2]["toolUseResult"]["persistedOutputPath"] = json!("/synthetic/output.txt")
            }
            "partial" => {
                values[2]["message"]["content"][0]["content"] = json!("PARTIAL view\n7\té")
            }
            "synthetic" => values[2]["isSynthetic"] = json!(true),
            _ => unreachable!(),
        }
        let parts = capture(&records(&values[..3]));
        let part = parts
            .iter()
            .find(|part| part.metadata.read_result.is_some())
            .unwrap();
        let result = part.metadata.read_result.as_ref().unwrap();
        assert!(result.returned_extent.is_none(), "{control}");
        assert_eq!(
            result.status,
            if control == "error" {
                JevReadStatus::Failed
            } else {
                JevReadStatus::Unknown
            }
        );
        assert_eq!(
            part.metadata.state,
            if control == "error" {
                JevOperationState::Error
            } else {
                JevOperationState::Unknown
            }
        );
        assert_eq!(result.truncated, matches!(control, "persisted" | "partial"));
    }
}

#[test]
fn native_result_bindings_reject_siblings_duplicate_calls_and_conflicting_source_ids() {
    for control in [
        "sibling",
        "duplicate-call",
        "source-call",
        "source-assistant",
        "other-session",
        "sidechain",
    ] {
        let mut values = native_values();
        match control {
            "sibling" => values[2]["parentUuid"] = values[0]["uuid"].clone(),
            "duplicate-call" => {
                let mut duplicate = values[1].clone();
                duplicate["uuid"] = json!(BRIDGE_RECORD);
                values.insert(2, duplicate);
            }
            "source-call" => values[2]["sourceToolUseID"] = json!("other"),
            "source-assistant" => values[2]["sourceToolAssistantUUID"] = json!(ROOT_RECORD),
            "other-session" => values[2]["sessionId"] = json!("other"),
            "sidechain" => values[2]["isSidechain"] = json!(true),
            _ => unreachable!(),
        }
        assert!(
            capture(&records(&values))
                .iter()
                .all(|part| part.metadata.read_result.is_none()),
            "{control}"
        );
    }
}

#[test]
fn retained_root_contract_rejects_loss_branches_compaction_and_other_producers() {
    for control in [
        "missing-root",
        "branch",
        "compaction",
        "version",
        "sidechain",
        "missing-parent",
        "missing-origin",
        "conflicting-replay",
    ] {
        let mut values = native_values();
        match control {
            "missing-root" => {
                values.remove(0);
            }
            "branch" => values[3]["parentUuid"] = values[0]["uuid"].clone(),
            "compaction" => values[1]["isCompactSummary"] = json!(true),
            "version" => {
                for value in &mut values {
                    value["version"] = json!("2.1.287");
                }
            }
            "sidechain" => {
                for value in &mut values {
                    value["isSidechain"] = json!(true);
                }
            }
            "missing-parent" => {
                values[1].as_object_mut().unwrap().remove("parentUuid");
            }
            "missing-origin" => {
                values[0].as_object_mut().unwrap().remove("origin");
            }
            "conflicting-replay" => {
                let mut replay = values[0].clone();
                replay["message"]["content"] = json!("Conflicting scope");
                values.insert(1, replay);
            }
            _ => unreachable!(),
        }
        let source = values
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let parts = capture(&source);
        assert!(
            parts
                .iter()
                .filter(|part| part.text == "Stop. Do not make further edits.")
                .all(|part| part.metadata.user_text_history.is_none()),
            "{control}"
        );
    }
}

#[test]
fn native_origin_controls_cannot_promote_injected_text_to_human_authority() {
    use antiburn_local::analysis::ContentAuthority;
    for (field, marker) in [
        ("origin", json!({"kind":"peer"})),
        ("promptSource", json!("system")),
        ("turnOrigin", json!("task_notification")),
        ("isMeta", json!(true)),
        ("isMeta", json!("false")),
        ("isSynthetic", json!(true)),
        ("planContent", json!("stale plan")),
    ] {
        let mut value = native_values().remove(0);
        value[field] = marker;
        let parts = capture(&value.to_string());
        assert!(
            parts
                .iter()
                .filter(|part| part.kind == ContentKind::UserText)
                .all(|part| part.authority == ContentAuthority::Unknown
                    && part.metadata.user_text_history.is_none()),
            "{field}"
        );
    }
}

#[test]
fn detected_root_loss_invalidates_the_publication_even_when_earlier_proofs_exist() {
    let mut values = native_values();
    values[3]["parentUuid"] = values[0]["uuid"].clone();
    let source = records(&values);
    assert!(
        capture(&source)
            .iter()
            .any(|part| part.metadata.user_text_history.is_some())
    );
    let input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        source: RawSource::Jsonl(source),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let mut collector = SessionCollector::new("claude", "scope-test");
    reader_for("claude").visit(&input, &mut collector).unwrap();
    assert!(
        collector
            .partial_reasons()
            .contains(&PartialReason::AttributionIncomplete)
    );
    let mut input = input;
    input.source = RawSource::Jsonl(records(&native_values()));
    input.fork_parent_session_id = Some("parent-session".into());
    let mut sink = ClaudeContentSink::default();
    reader_for("claude").visit(&input, &mut sink).unwrap();
    assert!(
        sink.contents
            .iter()
            .flat_map(|content| &content.parts)
            .all(|part| part.metadata.user_text_history.is_none())
    );
}

#[test]
fn exact_native_replay_keeps_scope_but_conflicting_replay_reports_loss() {
    let mut values = native_values();
    let replay = values[0].clone();
    values.insert(1, replay);
    let parts = capture(&records(&values));
    assert_eq!(
        parts
            .iter()
            .filter(|part| part.metadata.user_text_history.is_some())
            .count(),
        2
    );
    let mut input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        source: RawSource::Jsonl(records(&values)),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let mut collector = SessionCollector::new("claude", "scope-test");
    reader_for("claude").visit(&input, &mut collector).unwrap();
    assert!(
        !collector
            .partial_reasons()
            .contains(&PartialReason::AttributionIncomplete)
    );
    values[1]["message"]["content"] = json!("Different authority claim");
    input.source = RawSource::Jsonl(records(&values));
    let mut collector = SessionCollector::new("claude", "scope-test");
    reader_for("claude").visit(&input, &mut collector).unwrap();
    assert!(
        collector
            .partial_reasons()
            .contains(&PartialReason::AttributionIncomplete)
    );
}

#[test]
fn native_content_cap_denies_extent_and_digests_only_retained_output() {
    use antiburn_local::analysis::jev_evidence::JevReadStatus;
    let mut values = native_values();
    values[2]["message"]["content"][0]["content"] = json!("é".repeat(140_000));
    let parts = capture(&records(&values[..3]));
    let part = parts
        .iter()
        .find(|part| part.metadata.read_result.is_some())
        .unwrap();
    let result = part.metadata.read_result.as_ref().unwrap();
    assert!(part.truncated && result.truncated);
    assert_eq!(result.status, JevReadStatus::Unknown);
    assert!(result.returned_extent.is_none());
    assert_eq!(result.recorded_output_bytes, part.text.len() as u64);
    use sha2::{Digest, Sha256};
    assert_eq!(
        result.recorded_output_digest,
        format!("{:x}", Sha256::digest(part.text.as_bytes()))
    );
    assert!(part.metadata.bindings.is_empty());
}

fn informational_root_values() -> Vec<Value> {
    include_str!("../fixtures/claude_characterization/retained_informational_root.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn assert_user_binding(part: &ContentPart, record: &Value, pointer: &str) {
    use antiburn_local::analysis::jev_evidence::JevNativeFieldContainer;
    let proof = part.metadata.user_text_history.as_ref().unwrap();
    assert_eq!(Some(proof.message_id.as_str()), record["uuid"].as_str());
    let bindings: Vec<_> = part
        .metadata
        .bindings
        .iter()
        .filter(|binding| binding.field == JevInputField::UserMessage)
        .collect();
    assert_eq!(bindings.len(), 1);
    let binding = bindings[0];
    assert_eq!(binding.native_record_id.as_deref(), record["uuid"].as_str());
    assert_eq!(binding.container, JevNativeFieldContainer::Record);
    assert_eq!(binding.pointer, pointer);
    let text = record.pointer(pointer).unwrap().as_str().unwrap();
    assert_eq!((binding.start, binding.end), (0, text.len()));
    assert_eq!(
        text.get(binding.start..binding.end),
        Some(part.text.as_str())
    );
}

#[test]
fn observed_informational_root_binds_full_decoded_human_string() {
    let values = informational_root_values();
    let parts = capture(&records(&values));
    let human: Vec<_> = parts
        .iter()
        .filter(|part| part.metadata.user_text_history.is_some())
        .collect();
    assert_eq!(human.len(), 1);
    assert_user_binding(human[0], &values[1], "/message/content");
    assert_eq!(
        human[0].text,
        "Review é and \"quoted\" text.\nKeep the API unchanged."
    );
    resumed_parts(&values, &[1, 2]);
}

#[test]
fn human_text_blocks_bind_each_decoded_native_string_in_order() {
    for prelude in [false, true] {
        let mut values = if prelude {
            informational_root_values()
        } else {
            vec![native_values().remove(0)]
        };
        let index = values.len() - 1;
        values[index]["message"]["content"] = json!([
            {"type":"text", "text":"é\n\"quoted\"\\path"},
            {"type":"text", "text":"é\n\"quoted\"\\path"},
            {"type":"text", "text":""},
            {"type":"text", "text":"Last block 🦀"}
        ]);
        let parts = capture(&records(&values));
        let human: Vec<_> = parts
            .iter()
            .filter(|part| part.metadata.user_text_history.is_some())
            .collect();
        assert_eq!(human.len(), 4);
        for (block, part) in human.iter().enumerate() {
            assert_user_binding(
                part,
                &values[index],
                &format!("/message/content/{block}/text"),
            );
        }
        if prelude {
            resumed_parts(&values, &[1, 2]);
        }
    }
}

#[test]
fn rejected_human_blocks_never_publish_subset_proofs_or_bindings() {
    for invalid in [
        json!({"type":"image", "source":{"type":"base64", "data":"synthetic"}}),
        json!({"type":"text"}),
        json!({"type":"text", "text":"é".repeat(140_000)}),
    ] {
        let mut values = informational_root_values();
        values[1]["message"]["content"] = json!([
            {"type":"text", "text":"Intact first block"}, invalid
        ]);
        let parts = capture(&records(&values));
        assert!(parts.iter().all(|part| {
            part.metadata.user_text_history.is_none()
                && part
                    .metadata
                    .bindings
                    .iter()
                    .all(|binding| binding.field != JevInputField::UserMessage)
        }));
    }
}

#[test]
fn informational_prelude_does_not_admit_missing_ancestry_sdk_or_other_system_roots() {
    for control in [
        "missing-prelude",
        "subtype",
        "level",
        "meta",
        "sdk",
        "missing-origin",
        "missing-turn-origin",
    ] {
        let mut values = informational_root_values();
        match control {
            "missing-prelude" => {
                values.remove(0);
            }
            "subtype" => values[0]["subtype"] = json!("compact_boundary"),
            "level" => values[0]["level"] = json!("unknown"),
            "meta" => values[0]["isMeta"] = json!(true),
            "sdk" => {
                values[1]["promptSource"] = json!("sdk");
                values[1]["turnOrigin"] = json!("sdk");
            }
            "missing-origin" => {
                values[1].as_object_mut().unwrap().remove("origin");
            }
            "missing-turn-origin" => {
                values[1].as_object_mut().unwrap().remove("turnOrigin");
            }
            _ => unreachable!(),
        }
        assert!(
            capture(&records(&values))
                .iter()
                .all(|part| part.metadata.user_text_history.is_none()
                    && part
                        .metadata
                        .bindings
                        .iter()
                        .all(|binding| binding.field != JevInputField::UserMessage)),
            "{control}"
        );
    }
}

#[test]
fn stored_human_proof_keeps_native_user_message_binding() {
    use antiburn_local::analysis::jev::JevInputSelection;
    use antiburn_local::analysis::{
        FenceScope, TurnSessionKey, query_turn_content_offset_selected,
    };
    let values = informational_root_values();
    let input = SessionInput {
        agent: "claude".into(),
        session_id: "scope-test".into(),
        source: RawSource::Jsonl(records(&values)),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let store = MemoryTurnRowStore::new("claude", "scope-test");
    let mut sink = TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        "scope-test",
        None,
    );
    reader_for("claude").visit(&input, &mut sink).unwrap();
    store.with_connection(|connection| {
        let content = query_turn_content_offset_selected(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude",
                session_id: "scope-test",
            },
            &FenceScope::single(1),
            None,
            &Default::default(),
            0,
            JevInputSelection::from_fields(&[JevInputField::UserMessage]),
        )
        .unwrap();
        let human: Vec<_> = content
            .parts
            .iter()
            .filter(|item| item.part.metadata.user_text_history.is_some())
            .collect();
        assert_eq!(human.len(), 1);
        assert_user_binding(&human[0].part, &values[1], "/message/content");
    });
}
