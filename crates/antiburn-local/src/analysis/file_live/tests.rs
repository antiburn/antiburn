use super::*;

mod claude_admission;
mod codex_admission;
mod identity_proofs;
mod parser_upgrade;

mod native_admission_fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/native_admission_fixtures.rs"
    ));
}

fn assert_unsafe_message(
    message: &NativeMessageObservation,
    reason: super::super::ToolResultCompleteness,
) {
    match message {
        NativeMessageObservation::User(value) => {
            assert!(!value.context.main_session && !value.context.complete_through_event);
            assert_eq!(
                value.message.change,
                super::super::UserMessageChange::Present
            );
            assert_eq!(
                value.message.completeness,
                super::super::UserMessageCompleteness::Complete
            );
        }
        NativeMessageObservation::Public(value) => {
            assert!(!value.eligible);
            assert_eq!(value.completeness, reason);
            assert!(!value.context.as_ref().unwrap().main_session);
        }
        NativeMessageObservation::ToolResult(value) => assert_eq!(value.completeness, reason),
    }
}

#[test]
fn conflicting_pi_ids_and_claude_uuids_invalidate_public_evidence_on_every_page_and_restore() {
    for agent in ["claude", "pi"] {
        for kind in ["user", "call"] {
            for split in [false, true] {
                let directory = TempDir::new().unwrap();
                let [header, original, conflict, later] =
                    native_admission_fixtures::records(agent, kind);
                let format = if agent == "claude" {
                    SourceFormat::ClaudeJsonl
                } else {
                    SourceFormat::PiV3Jsonl
                };
                let input = input(&directory, agent, "facts", format, &header);
                let mut cursor = read_native_file_page(&input, None, &|| false)
                    .unwrap()
                    .cursor;
                append(&input, &original);
                if split {
                    let page =
                        read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
                    assert!(page.admission_failure.is_none());
                    assert!(page.evidence.events.iter().any(|event| event.live_eligible
                        && matches!(&event.fact, NativeEvidenceFact::Message(_))));
                    cursor = restored(&page.cursor);
                }
                append(&input, &conflict);
                let page =
                    read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
                let failure = page.admission_failure.as_ref().unwrap();
                assert_eq!(
                    failure.reason,
                    super::super::ToolResultCompleteness::UnsafeIdentity
                );
                assert!(
                    matches!(failure.position, super::super::ToolResultPosition::FileBytes { record_end, .. } if record_end == (header.len() + original.len() + conflict.len()) as u64)
                );
                assert_eq!(
                    page.evidence
                        .events
                        .iter()
                        .filter(|event| matches!(
                            event.fact,
                            NativeEvidenceFact::AdmissionFailure(_)
                        ))
                        .count(),
                    1
                );
                if split {
                    assert_eq!(page.evidence.events.len(), 1);
                }
                for event in &page.evidence.events {
                    if let NativeEvidenceFact::Message(message) = &event.fact {
                        assert_unsafe_message(message, failure.reason);
                        assert!(!event.live_eligible);
                    }
                }
                assert!(page.requests.is_empty());
                let next = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false)
                    .unwrap();
                assert_eq!(next.admission_failure.as_ref(), Some(failure));
                assert!(next.evidence.events.is_empty());
                append(&input, &later);
                let next = read_native_file_page(&input, Some(&restored(&next.cursor)), &|| false)
                    .unwrap();
                assert_eq!(next.admission_failure.as_ref(), Some(failure));
                assert!(next.requests.is_empty());
                assert!(next.evidence.events.iter().any(|event| matches!(
                    event.fact,
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(_))
                )));
                for event in &next.evidence.events {
                    if let NativeEvidenceFact::Message(message) = &event.fact {
                        assert_unsafe_message(message, failure.reason);
                        assert!(!event.live_eligible);
                    }
                }
            }
        }
    }
}

#[test]
fn observed_unsafe_admission_is_visible_before_output_continuation() {
    use serde_json::json;
    let directory = TempDir::new().unwrap();
    let [header, _, _, later] = native_admission_fixtures::records("claude", "call");
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        &header,
    );
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    let mut record: Value = serde_json::from_str(&later).unwrap();
    record["message"]["content"] = json!((0..MAX_PAGE_RESULTS + 100).map(|index| json!({"type":"tool_use","id":format!("call-{index}"),"name":"Bash","input":{"command":"true"}})).collect::<Vec<_>>());
    append(&input, &format!("{record}\n"));
    let mut page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
    assert!(page.has_more);
    assert_eq!(page.evidence.events.len(), MAX_PAGE_RESULTS);
    assert!(
        !page
            .evidence
            .events
            .iter()
            .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    let failure = page.admission_failure.clone().unwrap();
    assert_eq!(
        failure.reason,
        super::super::ToolResultCompleteness::UnsafeOrder
    );
    let mut delivered_failure = false;
    for _ in 0..3 {
        assert_eq!(page.admission_failure.as_ref(), Some(&failure));
        assert!(page.evidence.events.len() <= MAX_PAGE_RESULTS);
        assert!(serde_json::to_vec(&page.evidence).unwrap().len() <= MAX_PAGE_RESULT_BYTES);
        for event in &page.evidence.events {
            match &event.fact {
                NativeEvidenceFact::AdmissionFailure(_) => delivered_failure = true,
                NativeEvidenceFact::Message(message) => {
                    assert_unsafe_message(message, failure.reason);
                    assert!(!event.live_eligible);
                }
                _ => {}
            }
        }
        if !page.has_more {
            break;
        }
        page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    }
    assert!(!page.has_more && delivered_failure);
}

#[test]
fn failure_capacity_survives_a_full_record_output_buffer() {
    let mut sink = NativeSink::default();
    for index in 0..4096 {
        sink.native_activity(super::super::NativeActivityObservation {
            source_format: SourceFormat::PiV3Jsonl,
            session_id: "facts".into(),
            position: super::super::ToolResultPosition::Jsonl {
                record_index: index,
                item_index: 0,
            },
            token: index.to_string(),
            native_at_ms: None,
        });
    }
    assert_eq!(sink.events.len(), 4096);
    sink.push_failure(NativeAdmissionFailure {
        source_format: SourceFormat::PiV3Jsonl,
        session_id: "facts".into(),
        position: super::super::ToolResultPosition::Jsonl {
            record_index: 4096,
            item_index: u32::MAX,
        },
        reason: super::super::ToolResultCompleteness::UnsafeIdentity,
    });
    assert_eq!(sink.events.len(), 4096);
    assert!(matches!(
        sink.events.last(),
        Some(NativeEvidenceFact::AdmissionFailure(_))
    ));
    assert!(sink.result_bytes <= MAX_PENDING_EVIDENCE_BYTES);
}

#[test]
fn file_cursor_requires_valid_admission_state_and_v4_keeps_proven_unsafe_results() {
    use super::super::{ToolResultCompleteness, ToolResultPosition};
    let directory = TempDir::new().unwrap();
    let source = include_str!("../../../tests/fixtures/native_tool_facts/claude.jsonl");
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        source,
    );
    let page = read_native_file_page(&input, None, &|| false).unwrap();
    let mut missing = serde_json::to_value(&page.cursor).unwrap();
    missing.as_object_mut().unwrap().remove("admission_failure");
    assert!(serde_json::from_value::<NativeFileCursor>(missing).is_err());
    for reason in [
        ToolResultCompleteness::Complete,
        ToolResultCompleteness::Incomplete,
    ] {
        let mut cursor = page.cursor.clone();
        cursor.admission_failure.0 = Some(NativeAdmissionFailure {
            source_format: input.source_format,
            session_id: input.session_id.clone(),
            position: ToolResultPosition::FileBytes {
                record_start: 0,
                record_end: cursor.offset(),
                item_index: u32::MAX,
            },
            reason,
        });
        assert!(NativeFileCursor::decode(&cursor.encode()).is_err());
        assert!(read_native_file_page(&input, Some(&cursor), &|| false).is_err());
    }
    for reason in [
        ToolResultCompleteness::UnsafeIdentity,
        ToolResultCompleteness::UnsafeOrder,
        ToolResultCompleteness::UnsafeOwnership,
    ] {
        let mut cursor = page.cursor.clone();
        let mut result = page.results[0].observation.clone();
        result.position = ToolResultPosition::Jsonl {
            record_index: 2,
            item_index: 0,
        };
        result.completeness = reason;
        cursor.pending_results = Some(PendingResults {
            position: page.results[0].position,
            observations_json: [serde_json::to_string(&result).unwrap()].into(),
            requests: page.requests.clone(),
            main_session: true,
            history_complete: true,
        });
        let (binary, json) = super::legacy::encode_v4(&cursor);
        for converted in [
            NativeFileCursor::convert_v4(&binary).unwrap(),
            NativeFileCursor::convert_v4_json(&json).unwrap(),
        ] {
            let page =
                read_native_file_page(&input, Some(&restored(&converted)), &|| false).unwrap();
            assert_eq!(page.admission_failure.as_ref().unwrap().reason, reason);
            assert!(page.evidence.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(failure) if failure.reason == reason)));
            assert!(page.requests.is_empty());
            assert_eq!(page.results[0].observation.completeness, reason);
        }
    }
    let mut cursor = page.cursor;
    cursor.history_complete = false;
    let mut result = page.results[0].observation.clone();
    result.completeness = ToolResultCompleteness::UnsafeOwnership;
    result.position = ToolResultPosition::Jsonl {
        record_index: 2,
        item_index: 0,
    };
    result.context = None;
    cursor.pending_results = Some(PendingResults {
        position: page.results[0].position,
        observations_json: [serde_json::to_string(&result).unwrap()].into(),
        requests: Vec::new(),
        main_session: false,
        history_complete: false,
    });
    let (binary, _) = super::legacy::encode_v4(&cursor);
    assert!(
        NativeFileCursor::convert_v4(&binary)
            .unwrap()
            .admission_failure
            .0
            .is_none()
    );
}

#[test]
fn clipped_file_history_is_not_an_unsafe_admission() {
    for agent in ["claude", "pi"] {
        let directory = TempDir::new().unwrap();
        let [header, original, _, later] = native_admission_fixtures::records(agent, "user");
        let format = if agent == "claude" {
            SourceFormat::ClaudeJsonl
        } else {
            SourceFormat::PiV3Jsonl
        };
        let source = format!(
            "{header}{}{original}",
            "\n".repeat(NATIVE_FILE_STARTUP_BYTES as usize)
        );
        let input = input(&directory, agent, "facts", format, &source);
        let mut page = read_native_file_page(&input, None, &|| false).unwrap();
        while page.has_more {
            page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        }
        assert!(!page.history_complete && page.admission_failure.is_none());
        let mut later: Value = serde_json::from_str(&later).unwrap();
        later["type"] = serde_json::json!(if agent == "claude" { "user" } else { "message" });
        later["message"] = serde_json::json!({"role":"user","content":"A new complete request."});
        append(&input, &format!("{later}\n"));
        let page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert!(page.admission_failure.is_none());
        assert!(page.evidence.events.iter().any(|event| event.live_eligible && matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::User(value)) if value.context.main_session && !value.context.complete_through_event)));
    }
}

#[test]
fn named_v4_converters_preserve_source_progress_and_unresolved_joins() {
    for (agent, format, source) in [
        (
            "claude",
            SourceFormat::ClaudeJsonl,
            include_str!("../../../tests/fixtures/native_tool_facts/claude.jsonl"),
        ),
        (
            "codex",
            SourceFormat::CodexRolloutJsonl,
            include_str!("../../../tests/fixtures/native_tool_facts/codex.jsonl"),
        ),
        (
            "pi",
            SourceFormat::PiV3Jsonl,
            include_str!("../../../tests/fixtures/native_tool_facts/pi.jsonl"),
        ),
    ] {
        let directory = TempDir::new().unwrap();
        let records: Vec<_> = source.lines().collect();
        let prefix = format!("{}\n", records[..records.len() - 1].join("\n"));
        let input = input(&directory, agent, "facts", format, &prefix);
        let old = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let (binary, json) = super::legacy::encode_v4(&old);
        assert!(serde_json::from_slice::<NativeFileCursor>(&json).is_err());
        let expected = read_native_file_page(&input, Some(&old), &|| false).unwrap();
        for cursor in [
            NativeFileCursor::convert_v4(&binary).unwrap(),
            NativeFileCursor::convert_v4_json(&json).unwrap(),
            NativeFileCursor::decode(&binary).unwrap(),
        ] {
            assert_eq!(cursor.offset(), old.offset());
            assert_eq!(cursor.baseline_offset(), old.baseline_offset());
            assert_eq!(cursor.evidence_baseline_offset(), old.anchor.boundary);
            let retained_requests = if agent == "claude" {
                0
            } else {
                old.requests.len()
            };
            assert_eq!(cursor.requests.len(), retained_requests);
            assert_eq!(
                read_native_file_page(&input, Some(&cursor), &|| false)
                    .unwrap()
                    .requests
                    .len(),
                if agent == "claude" {
                    0
                } else {
                    expected.requests.len()
                }
            );
        }
        append(&input, &format!("{}\n", records.last().unwrap()));
        let page = read_native_file_page(
            &input,
            Some(&NativeFileCursor::convert_v4(&binary).unwrap()),
            &|| false,
        )
        .unwrap();
        assert_eq!(page.results.len(), 1, "{agent}");
        assert!(page.results[0].observation.facts.is_some(), "{agent}");
        assert!(page.results[0].live_eligible);
        let mut wrong = input.clone();
        wrong.session_id.push_str("-other");
        assert!(read_native_file_page(&wrong, Some(&page.cursor), &|| false).is_err());
    }
}

#[test]
fn v4_conversion_keeps_pending_results_and_partial_record_framing() {
    let directory = TempDir::new().unwrap();
    let source = include_str!("../../../tests/fixtures/native_tool_facts/codex.jsonl");
    let records: Vec<_> = source.lines().collect();
    let last = records.last().unwrap();
    let prefix = format!(
        "{}\n{}",
        records[..records.len() - 1].join("\n"),
        &last[..last.len() / 2]
    );
    let input = input(
        &directory,
        "codex",
        "facts",
        SourceFormat::CodexRolloutJsonl,
        &prefix,
    );
    let old = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    let (binary, json) = super::legacy::encode_v4(&old);
    append(&input, &format!("{}\n", &last[last.len() / 2..]));
    for converted in [
        NativeFileCursor::convert_v4(&binary).unwrap(),
        NativeFileCursor::convert_v4_json(&json).unwrap(),
    ] {
        let page = read_native_file_page(&input, Some(&converted), &|| false).unwrap();
        assert_eq!(page.results.len(), 1);
        assert!(page.results[0].live_eligible);
        assert!(page.evidence.events.iter().all(|event| event.live_eligible));
        let mut cursor = page.cursor;
        let mut result = page.results[0].observation.clone();
        result.position = super::super::ToolResultPosition::Jsonl {
            record_index: 10,
            item_index: 0,
        };
        result.context = None;
        cursor.pending_results = Some(PendingResults {
            position: page.results[0].position,
            observations_json: [serde_json::to_string(&result).unwrap()].into(),
            requests: cursor.requests.iter().cloned().collect(),
            main_session: true,
            history_complete: true,
        });
        cursor.baseline_offset = cursor.offset();
        let (binary, json) = super::legacy::encode_v4(&cursor);
        for converted in [
            NativeFileCursor::convert_v4(&binary).unwrap(),
            NativeFileCursor::convert_v4_json(&json).unwrap(),
        ] {
            let page = read_native_file_page(&input, Some(&converted), &|| false).unwrap();
            assert_eq!(page.bytes_read, 0);
            assert_eq!(page.results.len(), 1);
            assert!(!page.results[0].live_eligible);
            assert_eq!(page.evidence.events.len(), 1);
            assert!(!page.evidence.events[0].live_eligible);
        }
    }
}
use std::fs::OpenOptions;
use std::io::Write;
use tempfile::TempDir;

fn input(
    directory: &TempDir,
    agent: &str,
    session: &str,
    format: SourceFormat,
    text: &str,
) -> SessionInput {
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, text).unwrap();
    SessionInput {
        agent: agent.into(),
        session_id: session.into(),
        source: RawSource::File(path),
        source_format: format,
        fork_parent_session_id: None,
    }
}

fn append(input: &SessionInput, text: &str) {
    let RawSource::File(path) = &input.source else {
        unreachable!()
    };
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

fn restored(cursor: &NativeFileCursor) -> NativeFileCursor {
    NativeFileCursor::decode(&cursor.encode()).unwrap()
}

#[test]
fn pinned_claude_results_resume_without_metrics_or_replay() {
    let directory = TempDir::new().unwrap();
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        include_str!(
            "../../../tests/fixtures/claude_characterization/retained_native_results.jsonl"
        ),
    );
    let first = read_native_file_page(&input, None, &|| false).unwrap();
    assert_eq!(first.results.len(), 3);
    assert!(
        first.results.iter().all(|result| !result.live_eligible
            && result.observation.context.as_ref().unwrap().main_session)
    );
    let next = read_native_file_page(&input, Some(&restored(&first.cursor)), &|| false).unwrap();
    assert!(next.results.is_empty());
    assert_eq!(next.cursor.offset(), first.cursor.offset());
}

#[test]
fn first_page_retains_initialized_state_for_partial_publication_retry() {
    let directory = TempDir::new().unwrap();
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        include_str!(
            "../../../tests/fixtures/claude_characterization/retained_native_results.jsonl"
        ),
    );
    let first = read_native_file_page(&input, None, &|| false).unwrap();
    let initial = restored(first.initial_cursor.as_ref().unwrap());
    assert_eq!(initial.offset(), first.cursor.start_offset());
    assert_eq!(initial.baseline_offset(), first.baseline_offset);
    assert_eq!(
        initial.evidence_baseline_offset(),
        first.cursor.evidence_baseline_offset()
    );
    let retry = read_native_file_page(&input, Some(&initial), &|| false).unwrap();
    assert!(retry.initial_cursor.is_none());
    assert_eq!(
        serde_json::to_value(&retry.evidence).unwrap(),
        serde_json::to_value(&first.evidence).unwrap()
    );
    assert_eq!(retry.cursor.offset(), first.cursor.offset());
}

#[test]
fn unfinished_result_is_pending_across_restart_and_becomes_live() {
    let directory = TempDir::new().unwrap();
    let fixture = include_str!(
        "../../../tests/fixtures/claude_characterization/retained_native_results.jsonl"
    );
    let lines = fixture.split_inclusive('\n').collect::<Vec<_>>();
    let prefix = lines[..2].concat();
    let result = lines[2];
    let split = result.len() / 2;
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        &format!("{prefix}{}", &result[..split]),
    );
    let first = read_native_file_page(&input, None, &|| false).unwrap();
    assert!(first.pending_tail);
    assert!(first.results.is_empty());
    append(&input, &result[split..]);
    let next = read_native_file_page(&input, Some(&restored(&first.cursor)), &|| false).unwrap();
    assert_eq!(next.results.len(), 1);
    assert!(next.results[0].live_eligible);
    assert!(next.results[0].observation.arguments.is_some());
    assert!(!next.pending_tail);
    assert_eq!(next.results[0].position.record_start, prefix.len() as u64);
    assert_eq!(next.baseline_offset, first.baseline_offset);
}

#[test]
fn ordinary_append_during_read_does_not_discard_the_page() {
    use std::cell::Cell;
    let directory = TempDir::new().unwrap();
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        include_str!(
            "../../../tests/fixtures/claude_characterization/retained_native_results.jsonl"
        ),
    );
    let calls = Cell::new(0);
    let page = read_native_file_page(&input, None, &|| {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            append(&input, "\n");
        }
        false
    })
    .unwrap();
    assert_eq!(page.results.len(), 3);
}

#[test]
fn startup_and_every_continuation_read_have_fixed_byte_bounds() {
    let directory = TempDir::new().unwrap();
    let header = include_str!(
        "../../../tests/fixtures/claude_characterization/retained_native_results.jsonl"
    )
    .lines()
    .next()
    .unwrap();
    let input = input(
        &directory,
        "claude",
        "scope-test",
        SourceFormat::ClaudeJsonl,
        &format!(
            "{header}\n{}\n",
            " ".repeat(3 * NATIVE_FILE_STARTUP_BYTES as usize)
        ),
    );
    let mut page = read_native_file_page(&input, None, &|| false).unwrap();
    assert!(page.cursor.start_offset() > 0);
    assert_eq!(page.bytes_read, NATIVE_FILE_PAGE_BYTES);
    let mut total = page.bytes_read;
    let mut pages = 1;
    while page.has_more {
        page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
        total += page.bytes_read;
        pages += 1;
    }
    assert_eq!(total, NATIVE_FILE_STARTUP_BYTES);
    assert_eq!(pages, 4);
    assert!(!page.history_complete);
}

#[test]
fn pinned_pi_and_codex_results_keep_native_joins_and_request_context() {
    for (agent, session, format, fixture, expected) in [
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            include_str!("../../../tests/fixtures/pi_characterization/core_linear_v3.jsonl"),
            1,
        ),
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            include_str!(
                "../../../tests/fixtures/codex_characterization/paginated_completed.jsonl"
            ),
            3,
        ),
    ] {
        let directory = TempDir::new().unwrap();
        let input = input(&directory, agent, session, format, fixture);
        let page = read_native_file_page(&input, None, &|| false).unwrap();
        assert_eq!(page.results.len(), expected, "{agent}");
        for result in &page.results {
            assert!(result.observation.arguments.is_some(), "{agent}");
            assert!(matches!(
                result.observation.position,
                super::super::ToolResultPosition::FileBytes { .. }
            ));
            let context = result.observation.context.as_ref().unwrap();
            assert!(context.main_session, "{agent}");
            assert!(!context.user_messages.is_empty(), "{agent}");
            assert!(
                context
                    .user_messages
                    .iter()
                    .all(|message| message.is_bounded()
                        && matches!(
                            message.position,
                            super::super::UserMessagePosition::FileBytes { .. }
                        ))
            );
            assert_eq!(context.user_messages.len(), result.request_positions.len());
        }
        let resumed =
            read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert!(resumed.results.is_empty());
    }
}

fn ongoing_source(agent: &str, count: usize) -> (String, String, &'static str, SourceFormat) {
    use serde_json::json;
    let mut suffix = String::new();
    let (header, session, format) = match agent {
        "claude" => (
            include_str!("../../../tests/fixtures/claude_characterization/retained_native_results.jsonl").lines().next().unwrap().to_owned(),
            "scope-test", SourceFormat::ClaudeJsonl,
        ),
        "codex" => (
            json!({"timestamp":"2026-01-01T00:00:00Z", "type":"session_meta", "payload":{"id":"synthetic-root", "cli_version":"0.160.1", "thread_source":"user", "history_mode":"legacy"}}).to_string(),
            "synthetic-root", SourceFormat::CodexRolloutJsonl,
        ),
        "pi" => (
            include_str!("../../../tests/fixtures/pi_characterization/core_linear_v3.jsonl").lines().next().unwrap().to_owned(),
            "synthetic", SourceFormat::PiV3Jsonl,
        ),
        _ => unreachable!(),
    };
    let mut parent = if agent == "claude" {
        Some("00000000-0000-0000-0000-000000000001".to_owned())
    } else {
        None
    };
    for index in 0..count {
        let call = format!("call-{index}");
        let record = format!("request-{index}");
        let result = format!("result-{index}");
        let (request, output) = match agent {
            "claude" => (
                json!({"type":"assistant", "uuid":record, "parentUuid":parent, "sessionId":session, "version":"2.1.278", "entrypoint":"cli", "isSidechain":false, "message":{"role":"assistant", "content":[{"type":"tool_use", "id":call, "name":"Bash", "input":{"command":"synthetic test"}}]}}),
                json!({"type":"user", "uuid":result, "parentUuid":record, "sessionId":session, "version":"2.1.278", "entrypoint":"cli", "isSidechain":false, "message":{"role":"user", "content":[{"type":"tool_result", "tool_use_id":call, "content":"ok", "is_error":false}]}}),
            ),
            "codex" => (
                json!({"timestamp":"2026-01-01T00:00:04Z", "type":"response_item", "payload":{"type":"function_call", "call_id":call, "name":"exec_command", "arguments":"{\"cmd\":\"synthetic test\"}"}}),
                json!({"timestamp":"2026-01-01T00:00:05Z", "type":"response_item", "payload":{"type":"function_call_output", "id":result, "call_id":call, "status":"completed", "output":"ok"}}),
            ),
            "pi" => (
                json!({"type":"message", "id":record, "parentId":parent, "timestamp":"2026-01-01T00:00:04Z", "message":{"role":"assistant", "content":[{"type":"toolCall", "id":call, "name":"bash", "arguments":{"command":"synthetic test"}}]}}),
                json!({"type":"message", "id":result, "parentId":record, "timestamp":"2026-01-01T00:00:05Z", "message":{"role":"toolResult", "toolCallId":call, "toolName":"bash", "isError":false, "content":[{"type":"text", "text":"ok"}]}}),
            ),
            _ => unreachable!(),
        };
        suffix.push_str(&format!("{request}\n{output}\n"));
        parent = Some(result);
    }
    (format!("{header}\n"), suffix, session, format)
}

#[test]
fn all_file_sources_advance_beyond_4096_results_across_durable_pages() {
    for agent in ["claude", "codex", "pi"] {
        let directory = TempDir::new().unwrap();
        let (header, suffix, session, format) = ongoing_source(agent, 5000);
        let input = input(&directory, agent, session, format, &header);
        let mut cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let baseline = cursor.baseline_offset();
        append(&input, &suffix);
        let mut results = Vec::new();
        loop {
            let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
            assert_eq!(page.skipped_results, 0, "{agent}");
            assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
            assert_eq!(page.baseline_offset, baseline);
            assert!(
                page.results.iter().all(|result| result.live_eligible
                    && result.observation.arguments.is_some()
                    && result.observation.admission()
                        == super::super::ToolResultAdmission::Admitted),
                "{agent}"
            );
            results.extend(page.results.iter().map(|result| result.position.record_end));
            cursor = page.cursor;
            if !page.has_more {
                break;
            }
        }
        assert_eq!(results.len(), 5000, "{agent}");
        assert!(results.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(cursor.encode().len() < 2 * 1024 * 1024, "{agent}");
    }
}

#[test]
fn rewritten_consumed_tail_and_replaced_source_are_rejected() {
    let directory = TempDir::new().unwrap();
    let (header, _, session, format) = ongoing_source("pi", 0);
    let input = input(&directory, "pi", session, format, &header);
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    let RawSource::File(path) = &input.source else {
        unreachable!()
    };
    std::fs::write(path, header.replace("synthetic", "different")).unwrap();
    assert!(read_native_file_page(&input, Some(&cursor), &|| false).is_err());
    let replacement = directory.path().join("replacement.jsonl");
    std::fs::write(&replacement, &header).unwrap();
    std::fs::rename(replacement, path).unwrap();
    assert!(read_native_file_page(&input, Some(&cursor), &|| false).is_err());
}

#[test]
fn output_page_limits_retain_complete_record_results_across_restart() {
    use serde_json::json;
    let directory = TempDir::new().unwrap();
    let (header, _, session, format) = ongoing_source("claude", 0);
    let input = input(&directory, "claude", session, format, &header);
    let mut cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    let mut parent = "00000000-0000-0000-0000-000000000001".to_owned();
    let mut suffix = String::new();
    for index in 0..4 {
        let id = format!("large-request-{index}");
        let row = json!({"type":"user", "uuid":id, "parentUuid":parent, "sessionId":session, "version":"2.1.278", "entrypoint":"cli", "isSidechain":false, "message":{"role":"user", "content":"x".repeat(200_000)}});
        suffix.push_str(&format!("{row}\n"));
        parent = id;
    }
    let calls = (0..20).map(|index| json!({"type":"tool_use", "id":format!("tool-{index}"), "name":"Bash", "input":{"command":"synthetic test"}})).collect::<Vec<_>>();
    let results = (0..20).map(|index| json!({"type":"tool_result", "tool_use_id":format!("tool-{index}"), "content":"ok", "is_error":false})).collect::<Vec<_>>();
    let request = json!({"type":"assistant", "uuid":"batch-call", "parentUuid":parent, "sessionId":session, "version":"2.1.278", "entrypoint":"cli", "isSidechain":false, "message":{"role":"assistant", "content":calls}});
    let result = json!({"type":"user", "uuid":"batch-result", "parentUuid":"batch-call", "sessionId":session, "version":"2.1.278", "entrypoint":"cli", "isSidechain":false, "message":{"role":"user", "content":results}});
    suffix.push_str(&format!("{request}\n{result}\n"));
    append(&input, &suffix);
    let mut count = 0;
    let mut result_only_page = false;
    loop {
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert_eq!(page.skipped_results, 0);
        count += page.results.len();
        result_only_page |= page.bytes_read == 0 && !page.results.is_empty();
        for result in page.results {
            assert_eq!(
                result
                    .observation
                    .context
                    .as_ref()
                    .unwrap()
                    .user_messages
                    .len(),
                5
            );
            assert_eq!(result.request_positions.len(), 5);
        }
        cursor = page.cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(count, 20);
    assert!(result_only_page);
}

#[test]
fn retired_call_identity_cannot_rebind_after_restart_but_later_calls_work() {
    for agent in ["claude", "codex", "pi"] {
        let directory = TempDir::new().unwrap();
        let (header, suffix, session, format) = ongoing_source(agent, 4);
        let lines = suffix.lines().collect::<Vec<_>>();
        let initial = format!(
            "{}\n{}\n",
            lines[..4].join("\n"),
            lines[4].replace("call-2", "call-0")
        );
        let input = input(&directory, agent, session, format, &header);
        let baseline = read_native_file_page(&input, None, &|| false).unwrap();
        append(&input, &initial);
        let first = read_native_file_page(&input, Some(&baseline.cursor), &|| false).unwrap();
        assert_eq!(first.results.len(), 2);
        append(
            &input,
            &format!(
                "{}\n{}\n",
                lines[5].replace("call-2", "call-0"),
                lines[6..].join("\n")
            ),
        );
        let next =
            read_native_file_page(&input, Some(&restored(&first.cursor)), &|| false).unwrap();
        assert_eq!(next.results.len(), 2, "{agent}");
        assert!(next.results[0].observation.arguments.is_none(), "{agent}");
        assert!(next.results[1].observation.arguments.is_some(), "{agent}");
        assert_eq!(
            next.results[1].observation.admission(),
            super::super::ToolResultAdmission::Admitted,
            "{agent}"
        );
    }
}

#[test]
fn unresolved_calls_are_not_evicted_by_new_calls_at_the_retention_limit() {
    for agent in ["claude", "pi"] {
        let directory = TempDir::new().unwrap();
        let (header, suffix, session, format) = ongoing_source(agent, 1);
        let lines = suffix.lines().collect::<Vec<_>>();
        let mut request: Value = serde_json::from_str(lines[0]).unwrap();
        let block = request["message"]["content"][0].clone();
        request["message"]["content"] = Value::Array(
            (0..4097)
                .map(|index| {
                    let mut block = block.clone();
                    block["id"] = Value::String(format!("call-{index}"));
                    block
                })
                .collect(),
        );
        let input = input(&directory, agent, session, format, &header);
        let mut cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(&input, &format!("{request}\n"));
        loop {
            let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
            assert!(page.results.is_empty());
            cursor = page.cursor;
            if !page.has_more {
                break;
            }
        }
        append(&input, &format!("{}\n", lines[1]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert_eq!(page.results.len(), 1, "{agent}");
        assert!(page.results[0].observation.arguments.is_some(), "{agent}");
    }
}

#[test]
fn startup_baseline_finishes_even_when_more_bytes_arrive_during_catch_up() {
    let directory = TempDir::new().unwrap();
    let (header, _, session, format) = ongoing_source("claude", 0);
    let input = input(
        &directory,
        "claude",
        session,
        format,
        &format!(
            "{header}{}\n",
            " ".repeat(2 * NATIVE_FILE_STARTUP_BYTES as usize)
        ),
    );
    let mut page = read_native_file_page(&input, None, &|| false).unwrap();
    assert!(!page.baseline_reached);
    let baseline = page.baseline_offset;
    append(
        &input,
        &format!("{}\n", " ".repeat(NATIVE_FILE_STARTUP_BYTES as usize)),
    );
    while !page.baseline_reached {
        page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert_eq!(page.baseline_offset, baseline);
    }
    assert_eq!(page.cursor.offset(), baseline);
    assert!(page.has_more);
}

#[test]
fn cancelled_reads_preserve_the_prior_cursor_and_have_a_typed_error() {
    use std::cell::Cell;
    let directory = TempDir::new().unwrap();
    let (header, suffix, session, format) = ongoing_source("claude", 3);
    let input = input(&directory, "claude", session, format, &header);
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    let before = cursor.encode();
    append(&input, &suffix);
    let calls = Cell::new(0);
    let error = read_native_file_page(&input, Some(&cursor), &|| {
        calls.set(calls.get() + 1);
        calls.get() == 3
    })
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<NativeFileReadError>(),
        Some(&NativeFileReadError::Cancelled)
    );
    assert_eq!(cursor.encode(), before);
    let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
    assert_eq!(page.results.len(), 3);
}

#[test]
fn conflicting_record_identity_cannot_restore_its_old_pending_arguments() {
    for agent in ["claude", "pi"] {
        let directory = TempDir::new().unwrap();
        let (header, suffix, session, format) = ongoing_source(agent, 1);
        let lines = suffix.lines().collect::<Vec<_>>();
        let input = input(&directory, agent, session, format, &header);
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(&input, &format!("{}\n", lines[0]));
        let cursor = read_native_file_page(&input, Some(&cursor), &|| false)
            .unwrap()
            .cursor;
        append(
            &input,
            &format!(
                "{}\n{}\n",
                lines[0].replace("synthetic test", "different work"),
                lines[1]
            ),
        );
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert_eq!(page.results.len(), 1, "{agent}");
        assert!(page.results[0].observation.arguments.is_none(), "{agent}");
        assert!(!page.history_complete);
    }
}

#[test]
fn a_complete_malformed_record_blocks_unsafe_later_bindings() {
    for agent in ["claude", "codex", "pi"] {
        let directory = TempDir::new().unwrap();
        let (header, suffix, session, format) = ongoing_source(agent, 1);
        let lines = suffix.lines().collect::<Vec<_>>();
        let input = input(&directory, agent, session, format, &header);
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        append(&input, &format!("{}\n{{\n{}\n", lines[0], lines[1]));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert!(!page.history_complete);
        assert!(
            page.results
                .iter()
                .all(|result| result.observation.admission()
                    != super::super::ToolResultAdmission::Admitted),
            "{agent}"
        );
    }
}

#[test]
fn repeated_request_identity_keeps_one_exact_prefix_or_withholds_conflicting_text() {
    use serde_json::json;
    let fixture =
        include_str!("../../../tests/fixtures/codex_characterization/paginated_completed.jsonl");
    let rows = fixture
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    for conflict in [false, true] {
        let directory = TempDir::new().unwrap();
        let prefix = rows[..3]
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        let input = input(
            &directory,
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            &prefix,
        );
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let mut repeated = rows[2].clone();
        repeated["ordinal"] = json!(3);
        if conflict {
            repeated["payload"]["content"][0]["text"] = json!("Conflicting request text.");
        }
        let call = json!({"timestamp":"2026-10-07T10:00:04Z", "type":"response_item", "ordinal":4, "payload":{"type":"function_call", "call_id":"new-call", "name":"read_file", "arguments":"{\"path\":\"src/example.rs\"}"}});
        let output = json!({"timestamp":"2026-10-07T10:00:05Z", "type":"response_item", "ordinal":5, "payload":{"type":"function_call_output", "id":"new-result", "call_id":"new-call", "status":"completed", "output":"ok"}});
        append(&input, &format!("{repeated}\n{call}\n{output}\n"));
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert_eq!(page.results.len(), 1);
        let context = page.results[0].observation.context.as_ref().unwrap();
        assert_eq!(context.user_messages.len(), usize::from(!conflict));
        assert_eq!(context.complete_through_event, !conflict);
    }
}

#[test]
fn calls_in_a_conflicting_record_cannot_acquire_a_later_binding() {
    for agent in ["claude", "pi"] {
        let directory = TempDir::new().unwrap();
        let (header, suffix, session, format) = ongoing_source(agent, 3);
        let lines = suffix.lines().collect::<Vec<_>>();
        let input = input(&directory, agent, session, format, &header);
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let conflict = lines[0].replace("call-0", "rejected-call");
        append(&input, &format!("{}\n{conflict}\n", lines[0]));
        let cursor = read_native_file_page(&input, Some(&cursor), &|| false)
            .unwrap()
            .cursor;
        let mut call: Value = serde_json::from_str(lines[2]).unwrap();
        call[if agent == "claude" {
            "parentUuid"
        } else {
            "parentId"
        }] = Value::String("request-0".into());
        append(
            &input,
            &format!(
                "{}\n{}\n{}\n",
                call.to_string().replace("call-1", "rejected-call"),
                lines[3].replace("call-1", "rejected-call"),
                lines[4..].join("\n")
            ),
        );
        let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
        assert_eq!(page.results.len(), 2, "{agent}");
        assert!(page.results[0].observation.arguments.is_none(), "{agent}");
        assert!(page.results[1].observation.arguments.is_some(), "{agent}");
        assert_eq!(
            page.results[1].observation.admission(),
            super::super::ToolResultAdmission::UnsafeIdentity,
            "{agent}"
        );
    }
}

#[test]
fn a_large_pending_record_fits_the_persisted_json_cursor_limit() {
    let directory = TempDir::new().unwrap();
    let (header, suffix, session, format) = ongoing_source("claude", 1);
    let input = input(&directory, "claude", session, format, &header);
    let mut page = read_native_file_page(&input, None, &|| false).unwrap();
    append(
        &input,
        &format!(
            "{{\"padding\":\"{}\"}}",
            "x".repeat(super::super::MAX_RECORD_BYTES - 1024)
        ),
    );
    loop {
        page = read_native_file_page(&input, Some(&page.cursor), &|| false).unwrap();
        if !page.has_more {
            break;
        }
    }
    assert!(page.pending_tail);
    let json = serde_json::to_string(&page.cursor).unwrap();
    assert!(json.len() <= 32 * 1024 * 1024, "{}", json.len());
    let cursor: NativeFileCursor = serde_json::from_str(&json).unwrap();
    append(&input, &format!("\n{suffix}"));
    let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
    assert_eq!(page.results.len(), 1);
    assert!(page.results[0].observation.arguments.is_some());
}

#[test]
fn pi_results_do_not_share_the_unresolved_call_byte_budget() {
    let directory = TempDir::new().unwrap();
    let (header, suffix, session, format) = ongoing_source("pi", 4);
    let lines = suffix.lines().collect::<Vec<_>>();
    let input = input(&directory, "pi", session, format, &header);
    let mut cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    for (index, line) in lines.iter().step_by(2).enumerate() {
        let mut call: Value = serde_json::from_str(line).unwrap();
        if index > 0 {
            call["parentId"] = Value::String(format!("request-{}", index - 1));
        }
        call["message"]["content"][0]["arguments"] =
            serde_json::json!({"padding":"x".repeat(2 * 1024 * 1024 - 64)});
        append(&input, &format!("{call}\n"));
        loop {
            let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            cursor = page.cursor;
            if !page.has_more {
                break;
            }
        }
    }
    append(&input, &format!("{}\n", lines[7]));
    let page = read_native_file_page(&input, Some(&restored(&cursor)), &|| false).unwrap();
    assert_eq!(page.results.len(), 1);
    assert!(page.results[0].observation.arguments.is_some());
}
