use super::*;
use crate::analysis::{ToolResultCompleteness, ToolResultPosition};

const CANARY: &str = "LEGACY_HOST_USER_TEXT_MUST_NOT_RETURN";
const SOURCE: &str = include_str!(
    "../../../../tests/fixtures/claude_characterization/retained_native_results.jsonl"
);

fn legacy_cursor(unsafe_order: bool) -> (TempDir, SessionInput, NativeFileCursor, String) {
    let directory = TempDir::new().unwrap();
    let lines: Vec<_> = SOURCE.split_inclusive('\n').collect();
    let result = lines[4];
    let split = result.len() / 2;
    let source = format!("{}{}", lines[..4].concat(), &result[..split]);
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        &source,
    );
    let page = read_native_file_page(&input, None, &|| false).unwrap();
    assert_eq!(page.results.len(), 1);
    assert!(page.pending_tail);
    let mut cursor = page.cursor;
    let request = cursor.requests.front_mut().unwrap();
    request.message.parts[0].text = CANARY.into();
    cursor.request_bytes = CANARY.len();
    let request = request.clone();
    let mut queued = page.results[0].observation.clone();
    queued.call_id = Some("queued-result".into());
    queued.context.as_mut().unwrap().user_messages = vec![request.message.clone()];
    queued.position = ToolResultPosition::Jsonl {
        record_index: 2,
        item_index: 0,
    };
    cursor.pending_results = Some(PendingResults {
        position: page.results[0].position,
        observations_json: [serde_json::to_string(&queued).unwrap()].into(),
        requests: vec![request.clone()],
        main_session: true,
        history_complete: true,
    });
    queued.call_id = Some("queued-evidence-result".into());
    queued.position = page.results[0].observation.position;
    cursor.pending_evidence.0.push_back(NativeEvidenceEvent {
        fact: NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(queued)),
        live_eligible: false,
    });
    let user = user_observation(request.message.clone()).unwrap();
    cursor.pending_evidence.0.push_back(NativeEvidenceEvent {
        fact: NativeEvidenceFact::Message(NativeMessageObservation::User(user)),
        live_eligible: false,
    });
    let mut public = page
        .evidence
        .events
        .iter()
        .find_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::Public(public)) => {
                Some(public.clone())
            }
            _ => None,
        })
        .unwrap();
    public.context.as_mut().unwrap().user_messages = vec![request.message];
    cursor.pending_evidence.0.push_back(NativeEvidenceEvent {
        fact: NativeEvidenceFact::Message(NativeMessageObservation::Public(public)),
        live_eligible: false,
    });
    if unsafe_order {
        let failure = NativeAdmissionFailure {
            source_format: SourceFormat::ClaudeJsonl,
            session_id: input.session_id.clone(),
            position: page.results[0].observation.position,
            reason: ToolResultCompleteness::UnsafeOrder,
        };
        cursor.admission_failure.0 = Some(failure.clone());
        cursor.pending_evidence.0.push_back(NativeEvidenceEvent {
            fact: NativeEvidenceFact::AdmissionFailure(failure),
            live_eligible: false,
        });
    }
    assert!(cursor.valid());
    assert!(serde_json::to_string(&cursor).unwrap().contains(CANARY));
    (directory, input, cursor, result[split..].into())
}

fn assert_progress(before: &NativeFileCursor, after: &NativeFileCursor) {
    assert_eq!(after.offset(), before.offset());
    assert_eq!(after.start_offset(), before.start_offset());
    assert_eq!(after.baseline_offset(), before.baseline_offset());
    assert_eq!(
        after.evidence_baseline_offset(),
        before.evidence_baseline_offset()
    );
    assert_eq!(after.next_record_index, before.next_record_index);
    assert_eq!(
        postcard::to_allocvec(&after.framer).unwrap(),
        postcard::to_allocvec(&before.framer).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&after.adapter).unwrap(),
        serde_json::to_value(&before.adapter).unwrap()
    );
    assert_eq!(after.admission_failure.0, before.admission_failure.0);
}

#[test]
fn pre_65_claude_requests_leave_pending_output_without_text_or_result_replay() {
    for unsafe_order in [false, true] {
        let (_directory, input, old, suffix) = legacy_cursor(unsafe_order);
        let migrated = old.clone().migrate_pre_65_claude_requests().unwrap();
        assert_progress(&old, &migrated);
        assert!(migrated.requests.is_empty());
        assert!(!migrated.history_complete);
        assert!(!serde_json::to_string(&migrated).unwrap().contains(CANARY));
        assert!(migrated.pending_evidence.0.iter().all(|event| !matches!(
            event.fact,
            NativeEvidenceFact::Message(NativeMessageObservation::User(_))
        )));
        let restored_cursor = NativeFileCursor::decode(&migrated.encode()).unwrap();
        let restored_cursor: NativeFileCursor =
            serde_json::from_slice(&serde_json::to_vec(&restored_cursor).unwrap()).unwrap();
        assert_progress(&migrated, &restored_cursor);
        let idle = read_native_file_page(&input, Some(&restored_cursor), &|| false).unwrap();
        assert_eq!(idle.bytes_read, 0);
        assert_eq!(idle.results.len(), 2);
        assert!(idle.pending_tail);
        assert!(idle.results.iter().all(|result| matches!(
            result.observation.call_id.as_deref(),
            Some("queued-result" | "queued-evidence-result")
        )));
        assert!(idle.results.iter().all(|result| {
            let context = result.observation.context.as_ref().unwrap();
            context.user_messages.is_empty() && !context.complete_through_event
        }));
        assert!(
            !serde_json::to_string(&idle.evidence)
                .unwrap()
                .contains(CANARY)
        );
        assert_eq!(idle.admission_failure, old.admission_failure.0);
        append(&input, &suffix);
        let next = read_native_file_page(&input, Some(&restored(&idle.cursor)), &|| false).unwrap();
        assert_eq!(next.results.len(), 1);
        assert_eq!(
            next.results[0].observation.call_id.as_deref(),
            Some("test-1")
        );
        assert!(next.results[0].observation.arguments.is_some());
        assert_eq!(next.baseline_offset, old.baseline_offset());
        assert!(
            !serde_json::to_string(&next.evidence)
                .unwrap()
                .contains(CANARY)
        );
        let end = read_native_file_page(&input, Some(&restored(&next.cursor)), &|| false).unwrap();
        assert!(end.results.is_empty() && end.evidence.events.is_empty());
    }
}

#[test]
fn v4_conversion_removes_unknown_claude_request_origin_without_resetting_framing() {
    let (_directory, input, old, suffix) = legacy_cursor(false);
    let (binary, json) = super::super::legacy::encode_v4(&old);
    for converted in [
        NativeFileCursor::convert_v4(&binary).unwrap(),
        NativeFileCursor::convert_v4_json(&json).unwrap(),
        NativeFileCursor::decode(&binary).unwrap(),
    ] {
        assert_eq!(converted.offset(), old.offset());
        assert_eq!(converted.baseline_offset(), old.baseline_offset());
        assert_eq!(
            postcard::to_allocvec(&converted.framer).unwrap(),
            postcard::to_allocvec(&old.framer).unwrap()
        );
        assert!(converted.requests.is_empty());
        assert!(!converted.history_complete);
        assert!(!serde_json::to_string(&converted).unwrap().contains(CANARY));
        let page = read_native_file_page(&input, Some(&converted), &|| false).unwrap();
        assert_eq!(page.results.len(), 1);
        assert_eq!(
            page.results[0].observation.call_id.as_deref(),
            Some("queued-result")
        );
        assert!(
            page.results[0]
                .observation
                .context
                .as_ref()
                .unwrap()
                .user_messages
                .is_empty()
        );
    }
    append(&input, &suffix);
    let converted = NativeFileCursor::convert_v4(&binary).unwrap();
    let next = read_native_file_page(&input, Some(&converted), &|| false).unwrap();
    assert_eq!(next.results.len(), 2);
    assert!(
        next.results
            .iter()
            .all(|result| result.observation.call_id.as_deref() != Some("read-1"))
    );
}

#[test]
fn current_claude_cursor_restore_does_not_repeat_legacy_text_migration() {
    let (_directory, _input, cursor, _) = legacy_cursor(false);
    let restored = NativeFileCursor::decode(&cursor.encode()).unwrap();
    assert!(serde_json::to_string(&restored).unwrap().contains(CANARY));
    assert_eq!(restored.requests.len(), cursor.requests.len());
    assert_eq!(restored.history_complete, cursor.history_complete);
}

#[test]
fn main_revision_one_cursor_is_not_the_named_v4_predecessor() {
    let (_directory, _input, mut cursor, _) = legacy_cursor(false);
    cursor.revision = 1;

    assert!(!cursor.valid());
    assert!(NativeFileCursor::decode(&cursor.encode()).is_err());
    assert!(NativeFileCursor::convert_v4_json(&serde_json::to_vec(&cursor).unwrap()).is_err());
}

#[test]
fn parser_64_claude_state_and_v4_converter_share_the_historical_field_layout() {
    use std::collections::BTreeSet;
    let (_directory, _input, cursor, _) = legacy_cursor(false);
    let (_, v4) = super::super::legacy::encode_v4(&cursor);
    let current = serde_json::to_value(&cursor).unwrap();
    let v4: Value = serde_json::from_slice(&v4).unwrap();
    let fields = |value: &Value| {
        value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
    };
    let expected: BTreeSet<_> = [
        "scope",
        "retained_root",
        "user_message_root",
        "tool_identities",
        "max_usage_by_message_id",
        "context_window",
        "context_window_source",
        "first_model",
        "best_priceable",
        "last_seen_model",
        "skill_base_names",
        "pending_commands",
        "ordinal",
        "context",
        "threads",
        "seen_uuids",
        "session_id",
        "source_format",
        "native_calls",
        "native_call_bytes",
        "record_offset",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(fields(&current["adapter"]["Claude"]["parser"]), expected);
    assert_eq!(fields(&v4["adapter"]["Claude"]["parser"]), expected);
    for root in ["retained_root", "user_message_root"] {
        assert_eq!(
            fields(&current["adapter"]["Claude"]["parser"][root]),
            [
                "session",
                "last",
                "fingerprints",
                "invalid",
                "characterized",
                "user_slot"
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
    }
}

fn framing_only_legacy_cursor() -> (TempDir, SessionInput, NativeFileCursor) {
    use serde_json::json;

    let directory = TempDir::new().unwrap();
    let header = SOURCE.split_inclusive('\n').next().unwrap();
    let mut retained = json!({
        "type":"assistant", "uuid":"retained", "parentUuid":"omitted-parent",
        "sessionId":"scope-test", "version":"2.1.278", "entrypoint":"cli", "isSidechain":false,
        "message":{"role":"assistant","content":""}
    });
    let record_bytes = 300 * 1024;
    retained["message"]["content"] =
        json!("x".repeat(record_bytes - retained.to_string().len() - 1));
    let retained = format!("{retained}\n");
    assert_eq!(retained.len(), record_bytes);
    let discarded_bytes = 32 * 1024;
    let skipped = format!(
        "{}\n",
        "x".repeat(2 * NATIVE_FILE_STARTUP_BYTES as usize - 1)
    );
    let padding_bytes = NATIVE_FILE_STARTUP_BYTES as usize - discarded_bytes - record_bytes;
    let source = format!("{header}{skipped}{retained}{}", "\n".repeat(padding_bytes));
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        &source,
    );
    let first = read_native_file_page(&input, None, &|| false).unwrap();
    assert_eq!(first.bytes_read, NATIVE_FILE_PAGE_BYTES);
    assert!(first.has_more && first.pending_tail);
    assert!(
        first.results.is_empty() && first.evidence.events.is_empty() && first.requests.is_empty()
    );
    assert_eq!(first.cursor.next_record_index, 0);
    assert!(first.cursor.start_offset() > 0);
    assert_eq!(
        first.cursor.start_offset() + discarded_bytes as u64,
        (header.len() + skipped.len()) as u64
    );
    let mut historic = serde_json::to_value(first.cursor).unwrap();
    assert_eq!(historic["adapter"]["Claude"]["root"], true);
    assert!(historic["adapter"]["Claude"]["last"].is_null());
    historic["adapter"]["Claude"]["order_unproven"] = json!(false);
    let cursor: NativeFileCursor = serde_json::from_value(historic).unwrap();
    assert!(cursor.valid());
    (directory, input, restored(&cursor))
}

fn finish_framing_only_startup(
    input: &SessionInput,
    mut cursor: NativeFileCursor,
) -> NativeFilePage {
    loop {
        let page = read_native_file_page(input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
        if !page.has_more {
            assert!(page.baseline_reached && !page.pending_tail);
            return page;
        }
        cursor = page.cursor;
    }
}

fn append_new_claude_work(input: &SessionInput) {
    use serde_json::json;

    let mut records: Vec<Value> = SOURCE
        .lines()
        .take(3)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    records[0]["uuid"] = json!("live-request");
    records[0]["parentUuid"] = json!("retained");
    records[1]["uuid"] = json!("live-call");
    records[1]["parentUuid"] = json!("live-request");
    records[2]["uuid"] = json!("live-result");
    records[2]["parentUuid"] = json!("live-call");
    records[2]["sourceToolAssistantUUID"] = json!("live-call");
    append(
        input,
        &records
            .iter()
            .map(|record| format!("{record}\n"))
            .collect::<String>(),
    );
}

#[test]
fn pre_65_framing_only_claude_suffix_migration_keeps_progress_and_admits_partial_new_work() {
    let (_directory, input, historic) = framing_only_legacy_cursor();
    let migrated = historic.clone().migrate_pre_65_claude_requests().unwrap();
    let mut expected = serde_json::to_value(&historic).unwrap();
    expected["adapter"]["Claude"]["order_unproven"] = serde_json::json!(true);
    let expected: NativeFileCursor = serde_json::from_value(expected).unwrap();
    assert_progress(&expected, &migrated);
    assert!(!migrated.history_complete);
    let startup = finish_framing_only_startup(&input, migrated);
    assert!(startup.admission_failure.is_none() && !startup.history_complete);
    assert_eq!(startup.cursor.next_record_index, 1);
    assert!(startup.cursor.adapter.main_session());
    assert_eq!(startup.cursor.offset(), historic.baseline_offset());
    append_new_claude_work(&input);
    let next = read_native_file_page(&input, Some(&restored(&startup.cursor)), &|| false).unwrap();
    assert!(next.admission_failure.is_none() && !next.history_complete);
    assert_eq!(next.baseline_offset, historic.baseline_offset());
    assert_eq!(
        next.cursor.evidence_baseline_offset(),
        historic.evidence_baseline_offset()
    );
    assert_eq!(next.requests.len(), 1);
    assert_eq!(next.requests[0].message.message_id, "live-request");
    assert_eq!(next.results.len(), 1);
    let result = &next.results[0];
    assert!(result.live_eligible);
    assert_eq!(
        result.observation.admission(),
        crate::analysis::ToolResultAdmission::Admitted
    );
    let context = result.observation.context.as_ref().unwrap();
    assert!(context.main_session && !context.complete_through_event);
    assert_eq!(context.user_messages.len(), 1);
    assert_eq!(context.user_messages[0].message_id, "live-request");
}

#[test]
fn pre_65_framing_only_claude_suffix_migration_never_restores_invalid_roots_or_admission() {
    use serde_json::json;

    for control in [
        "invalid-user-root",
        "lost-header-root",
        "unsafe-identity",
        "unsafe-order",
        "unsafe-ownership",
    ] {
        let (_directory, input, mut historic) = framing_only_legacy_cursor();
        match control {
            "invalid-user-root" | "lost-header-root" => {
                let mut value = serde_json::to_value(&historic).unwrap();
                if control == "invalid-user-root" {
                    value["adapter"]["Claude"]["parser"]["user_message_root"]["invalid"] =
                        json!(true);
                } else {
                    value["adapter"]["Claude"]["root"] = json!(false);
                }
                historic = serde_json::from_value(value).unwrap();
            }
            _ => {
                historic.admission_failure.0 = Some(NativeAdmissionFailure {
                    source_format: SourceFormat::ClaudeJsonl,
                    session_id: input.session_id.clone(),
                    position: ToolResultPosition::FileBytes {
                        record_start: historic.start_offset(),
                        record_end: historic.offset(),
                        item_index: u32::MAX,
                    },
                    reason: match control {
                        "unsafe-identity" => ToolResultCompleteness::UnsafeIdentity,
                        "unsafe-order" => ToolResultCompleteness::UnsafeOrder,
                        "unsafe-ownership" => ToolResultCompleteness::UnsafeOwnership,
                        _ => unreachable!(),
                    },
                });
            }
        }
        let migrated = historic.clone().migrate_pre_65_claude_requests().unwrap();
        assert_progress(&historic, &migrated);
        let startup = finish_framing_only_startup(&input, migrated);
        append_new_claude_work(&input);
        let next =
            read_native_file_page(&input, Some(&restored(&startup.cursor)), &|| false).unwrap();
        assert_eq!(
            next.admission_failure, historic.admission_failure.0,
            "{control}"
        );
        assert!(
            !next.history_complete && next.requests.is_empty(),
            "{control}"
        );
        assert_eq!(next.results.len(), 1, "{control}");
        assert_ne!(
            next.results[0].observation.admission(),
            crate::analysis::ToolResultAdmission::Admitted,
            "{control}"
        );
    }
}

#[test]
fn pre_65_claude_suffix_start_requires_no_observed_records_and_a_pristine_user_root() {
    use serde_json::json;

    let (_directory, _input, historic) = framing_only_legacy_cursor();
    for control in [
        "full-prefix",
        "observed-record",
        "linked-state",
        "characterized-user-root",
    ] {
        let mut value = serde_json::to_value(&historic).unwrap();
        match control {
            "full-prefix" => value["start_offset"] = json!(0),
            "observed-record" => value["next_record_index"] = json!(1),
            "linked-state" => value["adapter"]["Claude"]["last"] = json!("prior"),
            "characterized-user-root" => {
                value["adapter"]["Claude"]["parser"]["user_message_root"]["characterized"] =
                    json!(true)
            }
            _ => unreachable!(),
        }
        let historic: NativeFileCursor = serde_json::from_value(value).unwrap();
        let migrated = historic.clone().migrate_pre_65_claude_requests().unwrap();
        assert_progress(&historic, &migrated);
    }
}
