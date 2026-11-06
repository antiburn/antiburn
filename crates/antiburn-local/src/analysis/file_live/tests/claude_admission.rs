use super::*;
use crate::analysis::{
    ContentAuthority, ContentKind, ContentPart, PublicMessageObservation, ToolResultAdmission,
    ToolResultCompleteness, ToolResultStatus, UserMessageCompleteness, UserMessageObservation,
    reader_for,
};
use serde_json::json;

fn record(id: &str, parent: Option<&str>, role: &str, content: Value) -> Value {
    json!({
        "type":role, "uuid":id, "parentUuid":parent, "sessionId":"facts",
        "version":"2.1.278", "entrypoint":"cli", "isSidechain":false,
        "message":{"role":role,"content":content}
    })
}

fn root() -> Value {
    record("root", None, "user", json!("Keep billing unchanged."))
}

fn sdk_root(version: &str) -> Value {
    let mut root = record("root", None, "user", json!("Use the existing checkout."));
    root["version"] = json!(version);
    root["entrypoint"] = json!("sdk-cli");
    root["userType"] = json!("external");
    root["promptSource"] = json!("sdk");
    root["turnOrigin"] = json!("sdk");
    root
}

fn sdk_lifecycle_prefix(session: &str) -> String {
    source(&[
        json!({"type":"session-owner","sessionId":session}),
        json!({"type":"queue-operation","sessionId":session}),
        json!({"type":"queue-operation","sessionId":session}),
    ])
}

#[test]
fn sdk_mixed_request_preserves_its_result_then_retires_prior_calls() {
    let directory = TempDir::new().unwrap();
    let mut call = read_call("call", "root", "read");
    let mut unused = call["message"]["content"][0].clone();
    unused["id"] = json!("unused");
    call["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(unused);
    let mut mixed = read_result("mixed", "call", "read");
    mixed["message"]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"text","text":"Continue with the next task."}));
    let mut records = vec![sdk_root("2.1.317"), call, mixed];
    for record in &mut records {
        record["version"] = json!("2.1.317");
        record["entrypoint"] = json!("sdk-cli");
        record["userType"] = json!("external");
    }
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        &source(&records[..2]),
    );
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    append(&input, &source(&records[2..]));
    let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
    assert!(page.admission_failure.is_none());
    let full = identity_proofs::full_events("claude", &source(&records));
    for events in [&full, &page.evidence.events] {
        let observed = results(events);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].tool_name.as_deref(), Some("Read"));
        assert!(observed[0].arguments.is_some());
        assert_eq!(observed[0].completeness, ToolResultCompleteness::Complete);
        assert!(!observed[0].context.as_ref().unwrap().complete_through_event);
    }
    let mut late = read_result("late", "call", "unused");
    late["version"] = json!("2.1.317");
    late["entrypoint"] = json!("sdk-cli");
    late["userType"] = json!("external");
    append(&input, &source(std::slice::from_ref(&late)));
    records.push(late);
    let resumed = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    assert!(resumed.admission_failure.is_some());
    let full = identity_proofs::full_events("claude", &source(&records));
    assert!(
        results(&full)
            .iter()
            .filter(|result| result.record_id.as_deref() == Some("late"))
            .all(|result| result.tool_name.is_none())
    );
}

#[test]
fn sdk_parallel_tool_results_keep_linked_source_continuity() {
    for reverse in [false, true] {
        let directory = TempDir::new().unwrap();
        let mut records = vec![sdk_root("2.1.317")];
        records.push(read_call("call-a", "root", "tool-a"));
        records.push(read_call("call-b", "call-a", "tool-b"));
        let results = [
            read_result("result-a", "call-a", "tool-a"),
            read_result("result-b", "call-b", "tool-b"),
        ];
        records.extend(if reverse {
            results.into_iter().rev().collect::<Vec<_>>()
        } else {
            results.to_vec()
        });
        for record in &mut records {
            record["version"] = json!("2.1.317");
            record["entrypoint"] = json!("sdk-cli");
            record["userType"] = json!("external");
        }
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&records[..1]),
        );
        let mut page = read_native_file_page(&input, None, &|| false).unwrap();
        for record in &records[1..] {
            append(&input, &source(std::slice::from_ref(record)));
            page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
            assert!(page.admission_failure.is_none(), "{reverse}: {record}");
            if record["type"] == "user" {
                assert_eq!(page.results.len(), 1);
                assert!(page.results[0].live_eligible);
                let context = page.results[0].observation.context.as_ref().unwrap();
                assert!(context.main_session, "{reverse}: {record}");
                assert!(!context.complete_through_event);
            }
        }
    }
}

#[test]
fn sdk_parallel_result_seams_reject_consumed_calls_and_later_requests() {
    for later_request in [false, true] {
        let directory = TempDir::new().unwrap();
        let mut records = vec![
            sdk_root("2.1.317"),
            read_call("call-a", "root", "tool-a"),
            read_call("call-b", "call-a", "tool-b"),
        ];
        if later_request {
            records.push(record(
                "request-b",
                Some("call-b"),
                "user",
                json!("New task."),
            ));
        } else {
            records.push(read_result("first-result", "call-a", "tool-a"));
        }
        records.push(read_result("unproved-result", "call-a", "tool-a"));
        for record in &mut records {
            record["version"] = json!("2.1.317");
            record["entrypoint"] = json!("sdk-cli");
            record["userType"] = json!("external");
        }
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&records[..1]),
        );
        let initial = read_native_file_page(&input, None, &|| false).unwrap();
        append(&input, &source(&records[1..records.len() - 1]));
        let page =
            read_native_file_page(&input, Some(&restored(&initial.cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none());
        append(&input, &source(&records[records.len() - 1..]));
        let rejected =
            read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert_eq!(
            rejected.admission_failure.unwrap().reason,
            ToolResultCompleteness::UnsafeOrder
        );
    }
}

fn read_call(id: &str, parent: &str, call: &str) -> Value {
    record(
        id,
        Some(parent),
        "assistant",
        json!([{
            "type":"tool_use", "id":call, "name":"Read",
            "input":{"file_path":"/synthetic/api.rs","offset":7,"limit":2}
        }]),
    )
}

fn read_result(id: &str, parent: &str, call: &str) -> Value {
    let mut result = record(
        id,
        Some(parent),
        "user",
        json!([{
            "type":"tool_result", "tool_use_id":call, "content":"7\tseven\n8\teight"
        }]),
    );
    result["toolUseResult"] = json!({"type":"text","file":{
        "filePath":"/synthetic/api.rs", "content":"seven\neight",
        "startLine":7, "numLines":2, "totalLines":20
    }});
    result
}

#[test]
fn characterized_sdk_cli_request_slot_reaches_normalized_live_evidence_with_unknown_history() {
    for version in ["2.1.309", "2.1.317"] {
        let directory = TempDir::new().unwrap();
        let text = if version == "2.1.309" {
            include_str!(
                "../../../../tests/fixtures/claude_characterization/sdk_cli_request_slot.jsonl"
            )
            .to_owned()
        } else {
            let mut root = sdk_root(version);
            root["uuid"] = json!("sdk-request");
            root["sessionId"] = json!("synthetic-sdk-session");
            let mut assistant = read_call("sdk-call", "sdk-request", "sdk-bash");
            assistant["sessionId"] = json!("synthetic-sdk-session");
            assistant["version"] = json!(version);
            assistant["entrypoint"] = json!("sdk-cli");
            let mut result = record(
                "sdk-result",
                Some("sdk-call"),
                "user",
                json!([{"type":"tool_result","tool_use_id":"sdk-bash","content":"ok","is_error":false}]),
            );
            result["sessionId"] = json!("synthetic-sdk-session");
            result["version"] = json!(version);
            result["entrypoint"] = json!("sdk-cli");
            source(&[root, assistant, result])
        };
        let (root_record, suffix) = text.split_once('\n').expect("SDK fixture has a root");
        let prefix = if version == "2.1.317" {
            sdk_lifecycle_prefix("synthetic-sdk-session")
        } else {
            String::new()
        };
        let input = input(
            &directory,
            "claude",
            "synthetic-sdk-session",
            SourceFormat::ClaudeJsonl,
            &format!("{prefix}{root_record}\n"),
        );
        let initial = read_native_file_page(&input, None, &|| false).unwrap();
        assert_eq!(
            initial.requests[0].message.message_id, "sdk-request",
            "{version}"
        );
        assert!(!initial.history_complete, "{version}");
        append(&input, suffix);
        let page =
            read_native_file_page(&input, Some(&restored(&initial.cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none(), "{version}");
        assert_eq!(
            page.requests
                .iter()
                .map(|request| request.message.message_id.as_str())
                .collect::<Vec<_>>(),
            ["sdk-request"],
            "{version}"
        );
        assert!(!page.history_complete, "{version}");
        assert_eq!(page.results.len(), 1, "{version}");
        let result = &page.results[0];
        assert!(result.live_eligible, "{version}");
        let result = &result.observation;
        let context = result.context.as_ref().unwrap();
        assert!(context.main_session, "{version}");
        assert!(!context.complete_through_event, "{version}");
        assert_eq!(
            request_ids(&context.user_messages),
            ["sdk-request"],
            "{version}"
        );
        assert!(
            initial.evidence.events.iter().any(|event| matches!(
                &event.fact,
                NativeEvidenceFact::Message(NativeMessageObservation::User(message))
                    if message.message.message_id == "sdk-request"
            )),
            "{version}"
        );
        let request_parts: Vec<_> = parts(&text)
            .into_iter()
            .filter(|part| part.text == "Use the existing checkout.")
            .collect();
        assert_eq!(request_parts.len(), 1, "{version}");
        assert!(
            request_parts.iter().all(|part| {
                part.authority == ContentAuthority::Unknown
                    && part.metadata.user_text_history.is_none()
            }),
            "{version}"
        );
    }
}

#[test]
fn sdk_cli_root_requires_exact_version_entrypoint_and_sdk_markers() {
    for case in [
        "unsupported-version",
        "legacy-prefix-version",
        "cli-entrypoint",
        "wrapper-entrypoint",
        "missing-turn-origin",
        "wrong-prompt-source",
        "wrong-user-type",
        "supplied-origin",
        "sidechain",
        "untrusted-record-before-root",
        "wrong-session-owner",
        "prefix-overflow",
    ] {
        let directory = TempDir::new().unwrap();
        let mut root = sdk_root("2.1.317");
        let owner_session = if case == "wrong-session-owner" {
            "different-session"
        } else {
            "facts"
        };
        let mut records = vec![
            json!({"type":"session-owner","sessionId":owner_session}),
            json!({"type":"queue-operation","sessionId":owner_session}),
        ];
        match case {
            "unsupported-version" => root["version"] = json!("2.1.318"),
            "legacy-prefix-version" => root["version"] = json!("2.1.309"),
            "cli-entrypoint" => root["entrypoint"] = json!("cli"),
            "wrapper-entrypoint" => root["entrypoint"] = json!("sdk-wrapper"),
            "missing-turn-origin" => {
                root.as_object_mut().unwrap().remove("turnOrigin");
            }
            "wrong-prompt-source" => root["promptSource"] = json!("typed"),
            "wrong-user-type" => root["userType"] = json!("internal"),
            "supplied-origin" => root["origin"] = json!({"kind":"sdk"}),
            "sidechain" => root["isSidechain"] = json!(true),
            "untrusted-record-before-root" => {
                records.push(record(
                    "before-root",
                    Some("parent"),
                    "assistant",
                    json!([]),
                ));
            }
            "wrong-session-owner" => {}
            "prefix-overflow" => {
                for _ in 0..16 {
                    records.push(json!({"type":"queue-operation","sessionId":owner_session}));
                }
            }
            _ => unreachable!(),
        }
        records.push(root);
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&records),
        );
        let page = read_native_file_page(&input, None, &|| false).unwrap();
        assert!(page.requests.is_empty(), "{case}");
        assert!(!page.history_complete, "{case}");
        assert!(page.results.is_empty(), "{case}");
    }
}

fn boundary(id: &str, parent: &str) -> Value {
    json!({
        "type":"system", "subtype":"compact_boundary", "uuid":id,
        "parentUuid":null, "logicalParentUuid":parent, "sessionId":"facts",
        "version":"2.1.278", "entrypoint":"cli", "isSidechain":false,
        "compactMetadata":{"trigger":"manual","preTokens":95000,"postTokens":15000}
    })
}

fn source(records: &[Value]) -> String {
    records.iter().map(|record| format!("{record}\n")).collect()
}

fn user_messages(events: &[NativeEvidenceEvent]) -> Vec<&UserMessageObservation> {
    events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::User(message)) => Some(message),
            _ => None,
        })
        .collect()
}

fn results(events: &[NativeEvidenceEvent]) -> Vec<&ToolResultObservation> {
    events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) => {
                Some(result)
            }
            _ => None,
        })
        .collect()
}

fn public_call<'a>(events: &'a [NativeEvidenceEvent], id: &str) -> &'a PublicMessageObservation {
    events
        .iter()
        .find_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::Public(call))
                if call.record_id == id =>
            {
                Some(call)
            }
            _ => None,
        })
        .unwrap()
}

fn request_ids(requests: &[UserMessage]) -> Vec<&str> {
    requests
        .iter()
        .map(|request| request.message_id.as_str())
        .collect()
}

fn parts(source: &str) -> Vec<ContentPart> {
    #[derive(Default)]
    struct Capture(Vec<ContentPart>);
    impl RecordSink for Capture {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::TurnContent(content) = record {
                self.0.extend(content.parts);
            }
        }
        fn finish(&mut self, _: SessionSummary) {}
    }
    let mut capture = Capture::default();
    reader_for("claude")
        .visit(
            &SessionInput {
                agent: "claude".into(),
                session_id: "facts".into(),
                source_format: SourceFormat::ClaudeJsonl,
                source: RawSource::Jsonl(source.into()),
                fork_parent_session_id: None,
            },
            &mut capture,
        )
        .unwrap();
    capture.0
}

#[test]
fn claude_host_injections_never_enter_live_or_full_request_history() {
    for marker in [
        json!({"isSynthetic":true}),
        json!({"isMeta":true}),
        json!({"planContent":"Change billing too."}),
        json!({"origin":{"kind":"task-notification"}}),
        json!({"origin":{"kind":"host"}}),
        json!({"origin":{"kind":"unknown"}}),
        json!({"origin":null}),
        json!({"origin":{}}),
        json!({"origin":"api"}),
        json!({"origin":{"kind":7}}),
        json!({"promptSource":"system"}),
        json!({"promptSource":null}),
        json!({"turnOrigin":"task_notification"}),
        json!({"turnOrigin":null}),
        json!({"origin":{"kind":"api"},"promptSource":"system","turnOrigin":"extension"}),
        json!({"origin":{"kind":"extension"},"isMeta":true}),
        json!({"origin":{"kind":"sdk"},"isSynthetic":true}),
    ] {
        let directory = TempDir::new().unwrap();
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&[root()]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let mut injected = record(
            "injected",
            Some("root"),
            "user",
            json!("Change billing too."),
        );
        injected
            .as_object_mut()
            .unwrap()
            .extend(marker.as_object().unwrap().clone());
        let records = [
            root(),
            injected.clone(),
            read_call("call", "injected", "read"),
            read_result("result", "call", "read"),
        ];
        append(&input, &source(&records[1..]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none(), "{marker}");
        assert_eq!(page.requests.len(), 1, "{marker}");
        assert!(user_messages(&page.evidence.events).is_empty(), "{marker}");
        let context = page.results[0].observation.context.as_ref().unwrap();
        assert_eq!(context.user_messages.len(), 1, "{marker}");
        assert_eq!(
            context.user_messages[0].parts[0].text,
            "Keep billing unchanged."
        );
        let full = identity_proofs::full_events("claude", &source(&records));
        assert_eq!(user_messages(&full).len(), 1, "{marker}");
        assert!(
            parts(&source(&records))
                .iter()
                .filter(|part| part.text == "Change billing too.")
                .all(|part| part.authority == ContentAuthority::Unknown
                    && part.metadata.user_text_history.is_none())
        );
    }
}

#[test]
fn claude_external_user_slot_origins_keep_unknown_human_authority() {
    for markers in [
        json!({"origin":{"kind":"api"},"promptSource":"extension","turnOrigin":"extension"}),
        json!({"origin":{"kind":"api"},"promptSource":"api","turnOrigin":"api"}),
        json!({"origin":{"kind":"extension"},"promptSource":"extension","turnOrigin":"extension"}),
        json!({"origin":{"kind":"sdk"},"promptSource":"sdk","turnOrigin":"sdk"}),
        json!({"promptSource":"sdk","turnOrigin":"sdk"}),
        json!({}),
    ] {
        let directory = TempDir::new().unwrap();
        let mut external = record(
            "external",
            Some("root"),
            "user",
            json!("Stop. Do not make further edits."),
        );
        external
            .as_object_mut()
            .unwrap()
            .extend(markers.as_object().unwrap().clone());
        let records = [
            root(),
            external,
            read_call("call", "external", "read"),
            read_result("result", "call", "read"),
        ];
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&records[..1]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(&input, &source(&records[1..]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none(), "{markers}");
        assert_eq!(page.requests.len(), 2, "{markers}");
        let full = identity_proofs::full_events("claude", &source(&records));
        assert_eq!(user_messages(&full).len(), 2, "{markers}");
        for events in [&full, &page.evidence.events] {
            let external = user_messages(events)
                .into_iter()
                .find(|message| message.message.message_id == "external")
                .unwrap();
            assert!(external.context.main_session, "{markers}");
            assert_eq!(
                external.message.parts[0].text,
                "Stop. Do not make further edits."
            );
            let observed = results(events);
            assert_eq!(
                request_ids(&observed[0].context.as_ref().unwrap().user_messages),
                ["root", "external"],
                "{markers}"
            );
        }
        let captured = parts(&source(&records));
        let external = captured
            .iter()
            .find(|part| {
                part.kind == ContentKind::UserText
                    && part.text == "Stop. Do not make further edits."
            })
            .unwrap();
        assert_eq!(external.authority, ContentAuthority::Unknown, "{markers}");
        assert!(external.metadata.user_text_history.is_none(), "{markers}");
    }
}

#[test]
fn claude_block_injections_do_not_gain_human_proof_from_the_envelope() {
    let mut mixed = root();
    mixed["origin"] = json!({"kind":"human"});
    mixed["promptSource"] = json!("typed");
    mixed["turnOrigin"] = json!("human");
    mixed["message"]["content"] = json!([
        {"type":"text","text":"Keep billing unchanged."},
        {"type":"text","text":"Change billing too.","isSynthetic":true}
    ]);
    let full = identity_proofs::full_events("claude", &source(&[mixed.clone()]));
    let messages = user_messages(&full);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].message.parts.len(), 1);
    assert_eq!(
        messages[0].message.completeness,
        UserMessageCompleteness::Incomplete
    );
    for part in parts(&source(&[mixed]))
        .iter()
        .filter(|part| part.kind == ContentKind::UserText)
    {
        assert_eq!(part.authority, ContentAuthority::Unknown);
        assert!(part.metadata.user_text_history.is_none());
    }
}

#[test]
fn claude_mixed_input_keeps_safe_text_without_human_authority() {
    let directory = TempDir::new().unwrap();
    let mut mixed = read_result("result", "call", "read");
    mixed["promptSource"] = json!("sdk");
    mixed["turnOrigin"] = json!("sdk");
    mixed["message"]["content"].as_array_mut().unwrap().extend([
        json!({"type":"text","text":"Check this image. Do not change billing."}),
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"synthetic"}}),
        json!({"type":"text","text":"Change billing too.","isSynthetic":true}),
    ]);
    let records = [
        root(),
        read_call("call", "root", "read"),
        mixed,
        read_call("later", "result", "later-read"),
    ];
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        &source(&records[..2]),
    );
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    append(&input, &source(&records[2..]));
    let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
    assert!(page.admission_failure.is_none());
    assert_eq!(page.requests.len(), 2);
    let message = &page.requests[1].message;
    assert_eq!(message.completeness, UserMessageCompleteness::Incomplete);
    assert_eq!(message.parts.len(), 1);
    assert_eq!(
        message.parts[0].text,
        "Check this image. Do not change billing."
    );
    assert!(message.is_bounded());
    assert!(!page.history_complete);
    assert_eq!(
        page.results[0].observation.status,
        ToolResultStatus::Success
    );
    assert_eq!(
        page.results[0].observation.admission(),
        ToolResultAdmission::Admitted
    );
    assert!(
        !page.results[0]
            .observation
            .context
            .as_ref()
            .unwrap()
            .complete_through_event
    );
    let full = identity_proofs::full_events("claude", &source(&records));
    let full_results = results(&full);
    let full_context = full_results[0].context.as_ref().unwrap();
    let live_context = page.results[0].observation.context.as_ref().unwrap();
    assert!(full_context.main_session && !full_context.complete_through_event);
    assert_eq!(request_ids(&full_context.user_messages), ["root"]);
    assert_eq!(request_ids(&live_context.user_messages), ["root"]);
    for events in [&full, &page.evidence.events] {
        let later = public_call(events, "later");
        let context = later.context.as_ref().unwrap();
        assert!(later.eligible && context.main_session && !context.complete_through_event);
        assert_eq!(request_ids(&context.user_messages), ["root", "result"]);
    }
    assert_eq!(
        user_messages(&full).last().unwrap().message.parts,
        message.parts
    );
    for part in parts(&source(&records))
        .iter()
        .filter(|part| part.kind == ContentKind::UserText)
    {
        assert_eq!(part.authority, ContentAuthority::Unknown);
        assert!(part.metadata.user_text_history.is_none());
    }
}

#[test]
fn claude_sdk_text_does_not_inherit_human_authority_from_a_mixed_envelope() {
    let mut mixed = root();
    mixed["origin"] = json!({"kind":"human"});
    mixed["promptSource"] = json!("typed");
    mixed["turnOrigin"] = json!("human");
    mixed["message"]["content"] = json!([
        {"type":"text","text":"Keep billing unchanged."},
        {"type":"text","text":"Review the API.","promptSource":"sdk","turnOrigin":"sdk"}
    ]);
    let text = source(&[mixed]);
    let full = identity_proofs::full_events("claude", &text);
    assert_eq!(user_messages(&full)[0].message.parts.len(), 2);
    for part in parts(&text)
        .iter()
        .filter(|part| part.kind == ContentKind::UserText)
    {
        assert_eq!(part.authority, ContentAuthority::Unknown);
        assert!(part.metadata.user_text_history.is_none());
    }
}

#[test]
fn claude_image_only_content_marks_full_and_live_context_partial_without_losing_admission() {
    for with_result in [false, true] {
        let directory = TempDir::new().unwrap();
        let image = json!({"type":"image","source":{
            "type":"base64","media_type":"image/png","data":"synthetic"
        }});
        let mut image_only = if with_result {
            read_result("image", "call", "read")
        } else {
            record("image", Some("call"), "user", json!([]))
        };
        image_only["message"]["content"]
            .as_array_mut()
            .unwrap()
            .push(image);
        let records = [
            root(),
            read_call("call", "root", "read"),
            image_only,
            read_call("later", "image", "later-read"),
            read_result("later-result", "later", "later-read"),
        ];
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&records[..2]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(&input, &source(&records[2..]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none());
        assert!(!page.history_complete);
        assert_eq!(page.requests.len(), 1);
        let full = identity_proofs::full_events("claude", &source(&records));
        for events in [&full, &page.evidence.events] {
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
            );
            assert!(!results(events).is_empty());
            for result in results(events) {
                let context = result.context.as_ref().unwrap();
                assert_eq!(result.admission(), ToolResultAdmission::Admitted);
                assert!(context.main_session && !context.complete_through_event);
                assert_eq!(request_ids(&context.user_messages), ["root"]);
            }
            let later = public_call(events, "later");
            let context = later.context.as_ref().unwrap();
            assert!(later.eligible && context.main_session && !context.complete_through_event);
            assert_eq!(request_ids(&context.user_messages), ["root"]);
        }
    }
}

#[test]
fn claude_same_record_text_never_enters_its_tool_result_request_prefix() {
    for text_first in [false, true] {
        let directory = TempDir::new().unwrap();
        let mut mixed = read_result("mixed", "call", "read");
        let text = json!({"type":"text","text":"Review another file next."});
        let content = mixed["message"]["content"].as_array_mut().unwrap();
        if text_first {
            content.insert(0, text);
        } else {
            content.push(text);
        }
        let records = [
            root(),
            read_call("call", "root", "read"),
            mixed,
            read_call("later", "mixed", "later-read"),
        ];
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&records[..2]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(&input, &source(&records[2..]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none() && page.history_complete);
        assert_eq!(page.requests.len(), 2);
        let full = identity_proofs::full_events("claude", &source(&records));
        for events in [&full, &page.evidence.events] {
            let observed = results(events);
            assert_eq!(observed.len(), 1);
            let result = observed[0];
            let context = result.context.as_ref().unwrap();
            assert_eq!(result.admission(), ToolResultAdmission::Admitted);
            assert!(context.complete_through_event);
            assert_eq!(
                request_ids(&context.user_messages),
                ["root"],
                "text_first={text_first}"
            );
            let later = public_call(events, "later");
            let context = later.context.as_ref().unwrap();
            assert!(later.eligible && context.complete_through_event);
            assert_eq!(request_ids(&context.user_messages), ["root", "mixed"]);
        }
    }
}

#[test]
fn claude_exact_short_identity_replays_preserve_full_and_live_pending_calls() {
    let directory = TempDir::new().unwrap();
    let initial = [root(), read_call("call", "root", "read")];
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        &source(&initial),
    );
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    let suffix = [
        root(),
        read_call("call", "root", "read"),
        read_result("result", "call", "read"),
        read_call("later", "result", "later-read"),
    ];
    append(&input, &source(&suffix));
    let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
    assert!(page.admission_failure.is_none() && page.history_complete);
    assert_eq!(page.requests.len(), 1);
    let mut records = initial.to_vec();
    records.extend(suffix);
    let full = identity_proofs::full_events("claude", &source(&records));
    assert_eq!(user_messages(&full).len(), 1);
    for events in [&full, &page.evidence.events] {
        assert!(
            !events
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
        );
        let observed = results(events);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].status, ToolResultStatus::Success);
        assert_eq!(observed[0].admission(), ToolResultAdmission::Admitted);
        assert!(public_call(events, "later").eligible);
    }
}

#[test]
fn claude_cold_tail_anchors_only_the_retained_chain_across_cursor_restarts() {
    let directory = TempDir::new().unwrap();
    let retained = record(
        "retained",
        Some("omitted-parent"),
        "user",
        json!("Review only the API."),
    );
    let text = format!(
        "{}{}{retained}\n",
        source(&[root()]),
        "\n".repeat(NATIVE_FILE_STARTUP_BYTES as usize)
    );
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        &text,
    );
    let mut page = read_native_file_page(&input, None, &|| false).unwrap();
    assert!(page.cursor.start_offset() > 0);
    let mut bytes = page.bytes_read;
    while page.has_more {
        page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
        bytes += page.bytes_read;
    }
    assert_eq!(bytes, NATIVE_FILE_STARTUP_BYTES);
    assert!(!page.history_complete && page.admission_failure.is_none());
    assert_eq!(page.requests.len(), 1);
    assert_eq!(page.requests[0].message.message_id, "retained");
    append(
        &input,
        &source(&[read_call("new-call", "retained", "new-read")]),
    );
    let call_page =
        read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    assert!(call_page.evidence.events.iter().any(|event| event.live_eligible
        && matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(message))
            if message.eligible && message.context.as_ref().unwrap().main_session)));
    let result = read_result("new-result", "new-call", "new-read").to_string();
    let split = result.len() / 2;
    append(&input, &result[..split]);
    let partial =
        read_native_file_page(&input, Some(&restored(&call_page.cursor)), &|| false).unwrap();
    assert!(partial.pending_tail && partial.results.is_empty());
    append(&input, &format!("{}\n", &result[split..]));
    let page = read_native_file_page(&input, Some(&restored(&partial.cursor)), &|| false).unwrap();
    assert!(page.admission_failure.is_none());
    assert_eq!(page.results.len(), 1);
    let result = &page.results[0];
    assert!(result.live_eligible);
    assert_eq!(result.observation.status, ToolResultStatus::Success);
    assert_eq!(
        result.observation.admission(),
        ToolResultAdmission::Admitted
    );
    let context = result.observation.context.as_ref().unwrap();
    assert!(!context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
    assert_eq!(context.user_messages[0].message_id, "retained");
    append(
        &input,
        &source(&[read_call("branch", "omitted-parent", "branch-read")]),
    );
    let unsafe_page =
        read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    assert_eq!(
        unsafe_page.admission_failure.unwrap().reason,
        ToolResultCompleteness::UnsafeOrder
    );
}

#[test]
fn claude_known_compaction_preserves_earlier_input_and_admits_new_work() {
    for split in [false, true] {
        let directory = TempDir::new().unwrap();
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&[root()]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(
            &input,
            &source(&[
                read_call("before", "root", "before-read"),
                read_result("before-result", "before", "before-read"),
            ]),
        );
        let before = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(before.history_complete);
        let earlier = before.results[0].observation.clone();
        assert_eq!(earlier.admission(), ToolResultAdmission::Admitted);
        append(&input, &source(&[boundary("boundary", "before-result")]));
        let cursor = if split {
            let page =
                read_native_file_page(&input, Some(&restored(&before.cursor)), &|| false).unwrap();
            assert!(page.admission_failure.is_none());
            assert!(!page.history_complete);
            restored(&page.cursor)
        } else {
            restored(&before.cursor)
        };
        let mut summary = record(
            "summary",
            Some("boundary"),
            "user",
            json!("Change billing too."),
        );
        summary["isCompactSummary"] = json!(true);
        let after = [
            summary,
            record(
                "request",
                Some("summary"),
                "user",
                json!("Continue the API review."),
            ),
            read_call("after", "request", "after-read"),
            read_result("after-result", "after", "after-read"),
        ];
        append(&input, &source(&after));
        let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        assert!(page.admission_failure.is_none());
        assert!(!page.history_complete);
        assert_eq!(page.requests.len(), 2);
        assert_eq!(page.requests[0].message.message_id, "root");
        assert_eq!(page.requests[1].message.message_id, "request");
        let result = &page.results[0];
        assert!(result.live_eligible);
        assert_eq!(
            result.observation.admission(),
            ToolResultAdmission::Admitted
        );
        let context = result.observation.context.as_ref().unwrap();
        assert!(!context.complete_through_event && context.main_session);
        assert_eq!(context.user_messages.len(), 2);
        assert!(earlier.context.as_ref().unwrap().complete_through_event);
        assert_eq!(earlier.admission(), ToolResultAdmission::Admitted);
        let mut records = vec![
            root(),
            read_call("before", "root", "before-read"),
            read_result("before-result", "before", "before-read"),
            boundary("boundary", "before-result"),
        ];
        records.extend(after);
        let full = identity_proofs::full_events("claude", &source(&records));
        assert!(
            !full
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
        );
        assert_eq!(user_messages(&full).len(), 2);
    }
}

#[test]
fn claude_cold_tail_does_not_hide_unsafe_first_record_ownership_or_cycles() {
    for control in ["other-session", "sidechain", "agent", "self-parent"] {
        let directory = TempDir::new().unwrap();
        let mut retained = record(
            "retained",
            Some("omitted-parent"),
            "user",
            json!("Review the API."),
        );
        match control {
            "other-session" => retained["sessionId"] = json!("other"),
            "sidechain" => retained["isSidechain"] = json!(true),
            "agent" => retained["agentId"] = json!("child"),
            "self-parent" => retained["parentUuid"] = json!("retained"),
            _ => unreachable!(),
        }
        let text = format!(
            "{}{}{retained}\n",
            source(&[root()]),
            "\n".repeat(NATIVE_FILE_STARTUP_BYTES as usize)
        );
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &text,
        );
        let mut page = read_native_file_page(&input, None, &|| false).unwrap();
        while page.has_more {
            page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        }
        assert_eq!(
            page.admission_failure.as_ref().unwrap().reason,
            if control == "self-parent" {
                ToolResultCompleteness::UnsafeOrder
            } else {
                ToolResultCompleteness::UnsafeOwnership
            },
            "{control}"
        );
        assert!(page.requests.is_empty());
    }
}

#[test]
fn claude_cold_tail_missing_evidence_stays_unavailable_without_an_unsafe_failure() {
    for missing in ["parentUuid", "uuid", "version", "isSidechain"] {
        let directory = TempDir::new().unwrap();
        let mut retained = record(
            "retained",
            Some("omitted-parent"),
            "user",
            json!("Review the API."),
        );
        retained.as_object_mut().unwrap().remove(missing);
        let text = format!(
            "{}{}{retained}\n",
            source(&[root()]),
            "\n".repeat(NATIVE_FILE_STARTUP_BYTES as usize)
        );
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &text,
        );
        let mut page = read_native_file_page(&input, None, &|| false).unwrap();
        while page.has_more {
            page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        }
        assert!(page.admission_failure.is_none(), "{missing}");
        assert!(page.requests.is_empty(), "{missing}");
        append(
            &input,
            &source(&[
                read_call("call", "retained", "read"),
                read_result("result", "call", "read"),
            ]),
        );
        let page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none(), "{missing}");
        assert!(page.requests.is_empty(), "{missing}");
        assert!(
            page.results
                .iter()
                .all(|result| result.observation.admission() != ToolResultAdmission::Admitted),
            "{missing}"
        );
    }
}

#[test]
fn claude_text_image_user_root_keeps_only_text_as_partial_user_slot_input() {
    let directory = TempDir::new().unwrap();
    let mut mixed = root();
    mixed["promptSource"] = json!("sdk");
    mixed["turnOrigin"] = json!("sdk");
    mixed["message"]["content"] = json!([
        {"type":"text","text":"Review this image."},
        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"synthetic"}}
    ]);
    let text = source(&[mixed]);
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        &text,
    );
    let page = read_native_file_page(&input, None, &|| false).unwrap();
    assert!(page.admission_failure.is_none());
    assert_eq!(page.requests.len(), 1);
    let message = &page.requests[0].message;
    assert_eq!(message.completeness, UserMessageCompleteness::Incomplete);
    assert_eq!(message.parts.len(), 1);
    assert_eq!(message.parts[0].text, "Review this image.");
    assert!(!page.history_complete);
    for part in parts(&text)
        .iter()
        .filter(|part| part.kind == ContentKind::UserText)
    {
        assert_eq!(part.authority, ContentAuthority::Unknown);
        assert!(part.metadata.user_text_history.is_none());
    }
}

#[test]
fn claude_unknown_compaction_links_stay_unsafe_after_restart() {
    for control in [
        "other-parent",
        "missing-logical-parent",
        "primary-parent",
        "wrong-type",
    ] {
        let directory = TempDir::new().unwrap();
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&[root()]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let mut compact = boundary("boundary", "root");
        match control {
            "other-parent" => compact["logicalParentUuid"] = json!("other"),
            "missing-logical-parent" => {
                compact.as_object_mut().unwrap().remove("logicalParentUuid");
            }
            "primary-parent" => compact["parentUuid"] = json!("root"),
            "wrong-type" => compact["type"] = json!("unknown"),
            _ => unreachable!(),
        }
        append(&input, &source(&[compact]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert_eq!(
            page.admission_failure.as_ref().unwrap().reason,
            ToolResultCompleteness::UnsafeOrder,
            "{control}"
        );
        assert!(page.requests.is_empty());
        let next = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert_eq!(next.admission_failure, page.admission_failure);
    }
}

#[test]
fn claude_read_completion_without_error_flag_requires_exact_producer_proof() {
    for control in [
        "accepted",
        "missing-payload",
        "other-path",
        "other-content",
        "other-start",
        "interrupted",
        "background",
        "persisted",
        "truncated",
        "partial",
        "synthetic",
        "source-call",
        "source-assistant",
        "ambiguous-results",
        "malformed-error-flag",
        "error",
    ] {
        let directory = TempDir::new().unwrap();
        let input = input(
            &directory,
            "claude",
            "facts",
            SourceFormat::ClaudeJsonl,
            &source(&[root(), read_call("call", "root", "read")]),
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let mut result = read_result("result", "call", "read");
        match control {
            "accepted" => {}
            "missing-payload" => {
                result.as_object_mut().unwrap().remove("toolUseResult");
            }
            "other-path" => result["toolUseResult"]["file"]["filePath"] = json!("/other"),
            "other-content" => result["toolUseResult"]["file"]["content"] = json!("other\neight"),
            "other-start" => result["toolUseResult"]["file"]["startLine"] = json!(6),
            "interrupted" => result["interruptedByShutdown"] = json!(true),
            "background" => result["toolUseResult"]["backgroundTaskId"] = json!("background"),
            "persisted" => {
                result["toolUseResult"]["persistedOutputPath"] = json!("/synthetic/output")
            }
            "truncated" => result["toolUseResult"]["truncated"] = json!(true),
            "partial" => {
                result["message"]["content"][0]["content"] = json!("PARTIAL view\n7\tseven")
            }
            "synthetic" => result["isSynthetic"] = json!(true),
            "source-call" => result["sourceToolUseID"] = json!("other"),
            "source-assistant" => result["sourceToolAssistantUUID"] = json!("other"),
            "ambiguous-results" => {
                let block = result["message"]["content"][0].clone();
                result["message"]["content"]
                    .as_array_mut()
                    .unwrap()
                    .push(block);
            }
            "malformed-error-flag" => result["message"]["content"][0]["is_error"] = json!("false"),
            "error" => result["message"]["content"][0]["is_error"] = json!(true),
            _ => unreachable!(),
        }
        append(&input, &source(&[result]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        let expected = match control {
            "accepted" => ToolResultStatus::Success,
            "error" => ToolResultStatus::Error,
            _ => ToolResultStatus::Unknown,
        };
        assert!(!page.results.is_empty(), "{control}");
        for result in &page.results {
            assert_eq!(result.observation.status, expected, "{control}");
            assert!(result.live_eligible, "{control}");
        }
    }
}

#[test]
fn claude_cowork_metrics_do_not_enable_cli_live_admission() {
    let directory = TempDir::new().unwrap();
    let fixture =
        include_str!("../../../../tests/fixtures/claude_characterization/cowork_transcript.jsonl");
    let records: Vec<_> = fixture
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect();
    let session = records[0]["sessionId"].as_str().unwrap();
    let input = input(
        &directory,
        "claude",
        session,
        SourceFormat::ClaudeJsonl,
        &source(&records[..2]),
    );
    let normalized = reader_for("claude")
        .normalize(&SessionInput {
            source: RawSource::Jsonl(fixture.into()),
            ..input.clone()
        })
        .unwrap();
    assert!(
        normalized
            .events
            .iter()
            .any(|event| event.usage.output_tokens > 0)
    );
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    append(&input, &source(&records[2..]));
    let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
    assert!(page.admission_failure.is_none());
    assert!(page.requests.is_empty());
    assert!(user_messages(&page.evidence.events).is_empty());
    assert!(
        page.results
            .iter()
            .all(|result| result.observation.admission() != ToolResultAdmission::Admitted)
    );
    assert!(page.evidence.events.iter().all(|event| !matches!(&event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::Public(message)) if message.eligible)));
}
