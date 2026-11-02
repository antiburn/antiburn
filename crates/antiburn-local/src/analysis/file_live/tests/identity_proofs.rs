use super::*;
use crate::analysis::{
    CompositeSink, EvidenceSource, SessionEvidenceAccumulator, SessionMetricsAccumulator,
    SourceCapabilities, SourceKind, ToolResultCompleteness, reader_for,
};

fn format_for(agent: &str) -> SourceFormat {
    match agent {
        "claude" => SourceFormat::ClaudeJsonl,
        "codex" => SourceFormat::CodexRolloutJsonl,
        _ => SourceFormat::PiV3Jsonl,
    }
}

pub(super) fn full_events(agent: &str, source: &str) -> Vec<NativeEvidenceEvent> {
    let mut sink = CompositeSink::new(
        SessionMetricsAccumulator::new(agent, "facts"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.into(),
            session_id: "facts".into(),
            kind: SourceKind::Jsonl,
            capabilities: match agent {
                "claude" => SourceCapabilities::claude(),
                "codex" => SourceCapabilities::codex(),
                _ => SourceCapabilities::pi(),
            },
        }),
    );
    reader_for(agent)
        .visit(
            &SessionInput {
                agent: agent.into(),
                session_id: "facts".into(),
                source_format: format_for(agent),
                source: RawSource::Jsonl(source.into()),
                fork_parent_session_id: None,
            },
            &mut sink,
        )
        .unwrap();
    sink.native_evidence().to_vec()
}

fn message_id(message: &NativeMessageObservation) -> &str {
    match message {
        NativeMessageObservation::User(value) => &value.message.message_id,
        NativeMessageObservation::Public(value) => &value.record_id,
        NativeMessageObservation::ToolResult(value) => value.record_id.as_deref().unwrap(),
    }
}

fn assert_admitted(message: &NativeMessageObservation) {
    match message {
        NativeMessageObservation::User(value) => assert!(value.context.main_session),
        NativeMessageObservation::Public(value) => {
            assert!(value.eligible);
            assert_eq!(value.completeness, ToolResultCompleteness::Complete);
            assert!(value.context.as_ref().unwrap().main_session);
        }
        NativeMessageObservation::ToolResult(value) => {
            assert_eq!(value.completeness, ToolResultCompleteness::Complete);
            assert!(value.context.as_ref().unwrap().main_session);
        }
    }
}

fn messages(events: &[NativeEvidenceEvent]) -> impl Iterator<Item = &NativeMessageObservation> {
    events.iter().filter_map(|event| match &event.fact {
        NativeEvidenceFact::Message(message) => Some(message),
        _ => None,
    })
}

#[test]
fn exact_claude_replays_keep_public_admission_on_same_separate_and_restored_pages() {
    for kind in ["user", "call"] {
        for split in [false, true] {
            for restore in [false, true] {
                let directory = TempDir::new().unwrap();
                let [header, original, _, later] =
                    native_admission_fixtures::records("claude", kind);
                let replay = original.replace("\":", "\" : ").replace(",\"", ", \"");
                assert_eq!(
                    serde_json::from_str::<Value>(&replay).unwrap(),
                    serde_json::from_str::<Value>(&original).unwrap()
                );
                let input = input(
                    &directory,
                    "claude",
                    "facts",
                    SourceFormat::ClaudeJsonl,
                    &header,
                );
                let baseline = read_native_file_page(&input, None, &|| false).unwrap();
                let mut cursor = baseline.cursor;
                let mut delivered = Vec::new();
                append(&input, &original);
                if split {
                    let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
                    delivered.extend(page.evidence.events);
                    cursor = page.cursor;
                }
                if restore {
                    cursor = restored(&cursor);
                }
                append(&input, &replay);
                let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
                assert!(page.admission_failure.is_none() && page.history_complete);
                if split {
                    assert!(page.evidence.events.is_empty());
                }
                delivered.extend(page.evidence.events);
                cursor = if restore {
                    restored(&page.cursor)
                } else {
                    page.cursor
                };
                append(&input, &later);
                let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
                assert!(page.admission_failure.is_none() && page.history_complete);
                delivered.extend(page.evidence.events);
                assert_eq!(messages(&delivered).count(), 2);
                for message in messages(&delivered) {
                    assert_admitted(message);
                }
                assert!(delivered.iter().all(|event| event.live_eligible));
                let full = full_events("claude", &format!("{header}{original}{replay}{later}"));
                assert!(
                    !full
                        .iter()
                        .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
                );
                for message in messages(&delivered) {
                    let full_message = messages(&full)
                        .find(|candidate| message_id(candidate) == message_id(message))
                        .unwrap();
                    assert_admitted(full_message);
                    match (message, full_message) {
                        (
                            NativeMessageObservation::Public(live),
                            NativeMessageObservation::Public(full),
                        ) => assert_eq!(live.payload, full.payload),
                        (
                            NativeMessageObservation::User(live),
                            NativeMessageObservation::User(full),
                        ) => assert_eq!(live.message.parts, full.message.parts),
                        _ => panic!("public message kind changed"),
                    }
                }
            }
        }
    }
}

#[test]
fn exact_claude_call_replay_keeps_the_original_result_join() {
    let source = include_str!("../../../../tests/fixtures/native_tool_facts/claude.jsonl");
    let rows = source.split_inclusive('\n').collect::<Vec<_>>();
    let directory = TempDir::new().unwrap();
    let input = input(
        &directory,
        "claude",
        "facts",
        SourceFormat::ClaudeJsonl,
        rows[0],
    );
    let cursor = read_native_file_page(&input, None, &|| false)
        .unwrap()
        .cursor;
    append(&input, rows[1]);
    let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
    append(&input, rows[1]);
    let replay = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    assert!(replay.evidence.events.is_empty() && replay.admission_failure.is_none());
    append(&input, rows[2]);
    let result = read_native_file_page(&input, Some(&restored(&replay.cursor)), &|| false).unwrap();
    assert_eq!(result.results.len(), 1);
    assert!(result.admission_failure.is_none());
    assert!(result.results[0].observation.arguments.is_some());
    assert_admitted(&NativeMessageObservation::ToolResult(
        result.results[0].observation.clone(),
    ));
}

#[test]
fn claude_replay_without_retained_content_proof_stays_soft_across_current_and_v4_restore() {
    for kind in ["user", "call"] {
        let directory = TempDir::new().unwrap();
        let [header, original, _, later] = native_admission_fixtures::records("claude", kind);
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
        append(&input, &original);
        let original_page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        let mut missing_order = serde_json::to_value(&original_page.cursor).unwrap();
        missing_order["adapter"]["Claude"]
            .as_object_mut()
            .unwrap()
            .remove("order_unproven");
        assert!(serde_json::from_value::<NativeFileCursor>(missing_order).is_err());
        let original_id = serde_json::from_str::<Value>(&original).unwrap()["uuid"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut json = serde_json::to_value(&original_page.cursor).unwrap();
        json["adapter"]["Claude"]["parser"]["user_message_root"]["fingerprints"]
            .as_object_mut()
            .unwrap()
            .remove(&original_id)
            .unwrap();
        let missing_proof: NativeFileCursor = serde_json::from_value(json).unwrap();
        append(&input, &original);
        let replay =
            read_native_file_page(&input, Some(&restored(&missing_proof)), &|| false).unwrap();
        assert!(replay.admission_failure.is_none() && !replay.history_complete);
        assert!(replay.evidence.events.is_empty());
        for message in messages(&original_page.evidence.events) {
            assert_admitted(message);
        }
        let (binary, json) = super::super::legacy::encode_v4(&replay.cursor);
        append(&input, &later);
        for cursor in [
            restored(&replay.cursor),
            NativeFileCursor::convert_v4(&binary).unwrap(),
            NativeFileCursor::convert_v4_json(&json).unwrap(),
        ] {
            let next = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert!(next.admission_failure.is_none() && !next.history_complete);
            assert_eq!(messages(&next.evidence.events).count(), 1);
            for message in messages(&next.evidence.events) {
                assert_admitted(message);
            }
            assert!(next.evidence.events.iter().all(|event| event.live_eligible));
        }
        let full = full_events("claude", &format!("{header}{original}{original}{later}"));
        assert!(
            !full
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
        );
        for message in messages(&full) {
            assert_admitted(message);
        }
    }
}

#[test]
fn proven_exact_pi_repeats_follow_the_full_readers_unique_id_rule() {
    for kind in ["user", "call"] {
        for split in [false, true] {
            let directory = TempDir::new().unwrap();
            let [header, original, _, later] = native_admission_fixtures::records("pi", kind);
            let input = input(&directory, "pi", "facts", SourceFormat::PiV3Jsonl, &header);
            let mut cursor = read_native_file_page(&input, None, &|| false)
                .unwrap()
                .cursor;
            append(&input, &original);
            if split {
                cursor = restored(
                    &read_native_file_page(&input, Some(&cursor), &|| false)
                        .unwrap()
                        .cursor,
                );
            }
            append(&input, &original);
            let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert_eq!(
                page.admission_failure.as_ref().unwrap().reason,
                ToolResultCompleteness::UnsafeIdentity
            );
            for message in messages(&page.evidence.events) {
                assert_unsafe_message(message, ToolResultCompleteness::UnsafeIdentity);
            }
            let full = full_events("pi", &format!("{header}{original}{original}{later}"));
            assert!(full.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(failure) if failure.reason == ToolResultCompleteness::UnsafeIdentity)));
        }
    }
}

#[test]
fn summary_false_positives_preserve_prior_and_later_public_admission() {
    for agent in ["claude", "pi"] {
        for kind in ["user", "call"] {
            for restore in [false, true] {
                let directory = TempDir::new().unwrap();
                let [header, original, _, uncertain] =
                    native_admission_fixtures::records(agent, kind);
                let mut unknown: Value = serde_json::from_str(&uncertain).unwrap();
                let id_field = if agent == "claude" { "uuid" } else { "id" };
                let unknown_id = unknown[id_field].as_str().unwrap().to_owned();
                let parent_field = if agent == "claude" {
                    "parentUuid"
                } else {
                    "parentId"
                };
                let mut before_summary = unknown.clone();
                before_summary[id_field] = Value::String(
                    if agent == "claude" {
                        "00000000-0000-0000-0000-000000000005"
                    } else {
                        "before-summary"
                    }
                    .into(),
                );
                before_summary["message"]["content"][0]["id"] =
                    Value::String("before-summary-call".into());
                unknown[parent_field] = before_summary[id_field].clone();
                let uncertain = format!("{unknown}\n");
                let mut later = unknown.clone();
                later[id_field] = Value::String(
                    if agent == "claude" {
                        "00000000-0000-0000-0000-000000000004"
                    } else {
                        "after-summary"
                    }
                    .into(),
                );
                later[parent_field] = Value::String(unknown_id.clone());
                later["message"]["content"][0]["id"] = Value::String("after-summary-call".into());
                let input = input(&directory, agent, "facts", format_for(agent), &header);
                let cursor = read_native_file_page(&input, None, &|| false)
                    .unwrap()
                    .cursor;
                append(&input, &original);
                let first = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
                let prior = messages(&first.evidence.events)
                    .cloned()
                    .collect::<Vec<_>>();
                assert_eq!(prior.len(), 1);
                assert_admitted(&prior[0]);
                let mut json = serde_json::to_value(&first.cursor).unwrap();
                let state = &mut json["adapter"][if agent == "claude" { "Claude" } else { "Pi" }];
                if agent == "claude" {
                    assert!(
                        state["parser"]["user_message_root"]["fingerprints"]
                            .get(&unknown_id)
                            .is_none()
                    );
                } else {
                    assert!(
                        !state["record_ids"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|id| id == unknown_id.as_str())
                    );
                }
                let bits = serde_json::from_value::<Vec<u8>>(state["records"].take()).unwrap();
                let mut filter = IdentityFilter::try_from(bits).unwrap();
                assert!(!filter.contains(&unknown_id));
                // Set a summary hit without a matching source record or exact cache entry.
                filter.insert(&unknown_id);
                assert!(filter.contains(&unknown_id));
                assert!(!filter.contains(before_summary[id_field].as_str().unwrap()));
                assert!(!filter.contains(later[id_field].as_str().unwrap()));
                state["records"] = serde_json::to_value(filter).unwrap();
                let cursor: NativeFileCursor = serde_json::from_value(json).unwrap();
                let cursor = if restore { restored(&cursor) } else { cursor };
                append(&input, &format!("{before_summary}\n{uncertain}"));
                let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
                assert!(page.admission_failure.is_none());
                assert!(!page.history_complete);
                assert_eq!(messages(&page.evidence.events).count(), 1);
                for message in messages(&page.evidence.events) {
                    assert_admitted(message);
                }
                assert!(page.evidence.events.iter().all(|event| event.live_eligible));
                for message in &prior {
                    assert_admitted(message);
                }
                append(&input, &format!("{later}\n"));
                let next = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false)
                    .unwrap();
                assert!(next.admission_failure.is_none() && !next.history_complete);
                let public = messages(&next.evidence.events).collect::<Vec<_>>();
                assert_eq!(public.len(), 1);
                assert_admitted(public[0]);
                assert!(next.evidence.events.iter().all(|event| event.live_eligible));
                assert!(
                    matches!(public[0], NativeMessageObservation::Public(value) if !value.context.as_ref().unwrap().complete_through_event)
                );
                let full = full_events(
                    agent,
                    &format!("{header}{original}{before_summary}\n{uncertain}{later}\n"),
                );
                assert!(
                    !full
                        .iter()
                        .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
                );
                for message in prior
                    .iter()
                    .chain(messages(&page.evidence.events))
                    .chain(public)
                {
                    assert_admitted(
                        messages(&full)
                            .find(|candidate| message_id(candidate) == message_id(message))
                            .unwrap(),
                    );
                }
            }
        }
    }
}

#[test]
fn pi_v4_conversion_retains_only_exact_owned_identity_proofs() {
    for kind in ["user", "call"] {
        let directory = TempDir::new().unwrap();
        let [header, original, conflict, later] = native_admission_fixtures::records("pi", kind);
        let source = format!(
            "{header}{original}{}",
            if kind == "call" { &later } else { "" }
        );
        let input = input(&directory, "pi", "facts", SourceFormat::PiV3Jsonl, &source);
        let cursor = read_native_file_page(&input, None, &|| false)
            .unwrap()
            .cursor;
        let mut missing = serde_json::to_value(&cursor).unwrap();
        missing["adapter"]["Pi"]
            .as_object_mut()
            .unwrap()
            .remove("record_ids");
        assert!(serde_json::from_value::<NativeFileCursor>(missing).is_err());
        let mut wrong_bytes = serde_json::to_value(&cursor).unwrap();
        wrong_bytes["adapter"]["Pi"]["record_id_bytes"] = serde_json::json!(0);
        let wrong_bytes: NativeFileCursor = serde_json::from_value(wrong_bytes).unwrap();
        assert!(NativeFileCursor::decode(&wrong_bytes.encode()).is_err());
        let (binary, json) = super::super::legacy::encode_v4(&cursor);
        append(&input, &conflict);
        for old in [
            NativeFileCursor::convert_v4(&binary).unwrap(),
            NativeFileCursor::convert_v4_json(&json).unwrap(),
        ] {
            assert_eq!(old.offset(), cursor.offset());
            let page = read_native_file_page(&input, Some(&restored(&old)), &|| false).unwrap();
            assert_eq!(
                page.admission_failure.as_ref().unwrap().reason,
                ToolResultCompleteness::UnsafeIdentity
            );
        }
    }
}

#[test]
fn saturated_pi_identity_cache_keeps_unresolved_owner_conflicts_hard_across_restore() {
    let directory = TempDir::new().unwrap();
    let [prefix, call, between, conflict] =
        native_admission_fixtures::saturated_pi_call_owner_conflict();
    let input = input(&directory, "pi", "facts", SourceFormat::PiV3Jsonl, &prefix);
    let mut page = read_native_file_page(&input, None, &|| false).unwrap();
    while page.has_more {
        page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    }
    let state = serde_json::to_value(&page.cursor).unwrap();
    assert_eq!(
        state["adapter"]["Pi"]["record_ids"]
            .as_array()
            .unwrap()
            .len(),
        4096
    );
    assert!(
        state["adapter"]["Pi"]["parser"]["native_tool_calls"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    append(&input, &call);
    let owner = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    assert!(owner.admission_failure.is_none());
    assert_eq!(messages(&owner.evidence.events).count(), 1);
    for message in messages(&owner.evidence.events) {
        assert_admitted(message);
    }
    assert!(
        owner
            .evidence
            .events
            .iter()
            .all(|event| event.live_eligible)
    );
    append(&input, &between);
    let page = read_native_file_page(&input, Some(&restored(&owner.cursor)), &|| false).unwrap();
    assert!(page.admission_failure.is_none());
    let state = serde_json::to_value(&page.cursor).unwrap();
    assert!(
        !state["adapter"]["Pi"]["record_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "owner-a")
    );
    assert_eq!(state["adapter"]["Pi"]["last"], "between-b");
    assert_eq!(
        state["adapter"]["Pi"]["parser"]["native_tool_calls"]["unresolved-call"]["record_id"],
        "owner-a"
    );
    let current_json: NativeFileCursor =
        serde_json::from_slice(&serde_json::to_vec(&page.cursor).unwrap()).unwrap();
    let (v4_binary, v4_json) = super::super::legacy::encode_v4(&page.cursor);
    let before = page.cursor.offset();
    append(&input, &conflict);
    for cursor in [
        page.cursor.clone(),
        restored(&page.cursor),
        current_json,
        NativeFileCursor::convert_v4(&v4_binary).unwrap(),
        NativeFileCursor::convert_v4_json(&v4_json).unwrap(),
    ] {
        assert_eq!(cursor.offset(), before);
        let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        assert!(!page.has_more);
        let failure = page.admission_failure.as_ref().unwrap();
        assert_eq!(failure.reason, ToolResultCompleteness::UnsafeIdentity);
        assert_eq!(page.evidence.events.iter().filter(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(value) if value == failure)).count(), 1);
        assert!(messages(&page.evidence.events).next().is_none());
        let state = serde_json::to_value(&page.cursor).unwrap();
        assert!(
            state["adapter"]["Pi"]["parser"]["native_tool_calls"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        let next = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert_eq!(next.admission_failure.as_ref(), Some(failure));
    }
    let full = full_events("pi", &format!("{prefix}{call}{between}{conflict}"));
    assert!(full.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(failure) if failure.reason == ToolResultCompleteness::UnsafeIdentity)));
    assert!(messages(&full).any(|message| matches!(message, NativeMessageObservation::Public(value) if value.record_id == "owner-a" && !value.eligible && value.completeness == ToolResultCompleteness::UnsafeIdentity && !value.context.as_ref().unwrap().main_session)));
}

#[test]
fn saturated_pi_identity_cache_keeps_retained_requests_hard_across_pages_and_all_restores() {
    for split in [false, true] {
        let directory = TempDir::new().unwrap();
        let [prefix, request, between, conflict] =
            native_admission_fixtures::saturated_pi_request_conflict();
        let input = input(&directory, "pi", "facts", SourceFormat::PiV3Jsonl, &prefix);
        let mut page = read_native_file_page(&input, None, &|| false).unwrap();
        while page.has_more {
            page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        }
        assert!(page.admission_failure.is_none() && page.history_complete);
        let mut cursor = page.cursor;
        append(&input, &format!("{request}{between}"));
        if split {
            let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert!(page.admission_failure.is_none() && page.history_complete);
            assert_eq!(page.requests.len(), 1);
            assert_eq!(page.requests[0].message.message_id, "retained-user");
            assert_eq!(messages(&page.evidence.events).count(), 1);
            assert!(page.evidence.events.iter().all(|event| event.live_eligible));
            assert_admitted(messages(&page.evidence.events).next().unwrap());
            cursor = page.cursor;
        }
        let json = serde_json::to_value(&cursor).unwrap();
        let ids = json["adapter"]["Pi"]["record_ids"].as_array().unwrap();
        assert_eq!(ids.len(), 4096);
        assert!(!ids.iter().any(|id| id == "retained-user"));
        assert!(
            json["adapter"]["Pi"]["parser"]["native_tool_calls"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        let (v4_binary, v4_json) = super::super::legacy::encode_v4(&cursor);
        append(&input, &conflict);
        for cursor in [
            cursor.clone(),
            restored(&cursor),
            serde_json::from_value(json).unwrap(),
            NativeFileCursor::convert_v4(&v4_binary).unwrap(),
            NativeFileCursor::convert_v4_json(&v4_json).unwrap(),
        ] {
            let before = cursor.offset();
            let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert!(page.cursor.offset() > before);
            assert!(!page.has_more && !page.history_complete);
            assert!(page.requests.is_empty());
            let failure = page.admission_failure.as_ref().unwrap();
            assert_eq!(failure.reason, ToolResultCompleteness::UnsafeIdentity);
            assert!(
                matches!(failure.position, crate::analysis::ToolResultPosition::FileBytes { record_start, record_end, .. } if record_start == (prefix.len() + request.len() + between.len()) as u64 && record_end == page.cursor.offset())
            );
            assert_eq!(page.evidence.events.iter().filter(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(value) if value == failure)).count(), 1);
            for message in messages(&page.evidence.events) {
                assert_unsafe_message(message, ToolResultCompleteness::UnsafeIdentity);
            }
            let next =
                read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
            assert_eq!(next.admission_failure.as_ref(), Some(failure));
        }
        let full = full_events("pi", &format!("{prefix}{request}{between}{conflict}"));
        assert!(full.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(failure) if failure.reason == ToolResultCompleteness::UnsafeIdentity)));
        for message in messages(&full) {
            assert_unsafe_message(message, ToolResultCompleteness::UnsafeIdentity);
        }
    }
}

#[test]
fn saturated_pi_summary_only_repeats_stay_soft_without_filter_changes() {
    let directory = TempDir::new().unwrap();
    let [prefix, request, between, conflict] =
        native_admission_fixtures::saturated_pi_request_conflict();
    let mut label: Value = serde_json::from_str(&request).unwrap();
    label["type"] = serde_json::json!("label");
    label.as_object_mut().unwrap().remove("message");
    let source = format!("{prefix}{label}\n{between}");
    let input = input(&directory, "pi", "facts", SourceFormat::PiV3Jsonl, &source);
    let mut page = read_native_file_page(&input, None, &|| false).unwrap();
    while page.has_more {
        page = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
    }
    assert!(page.admission_failure.is_none() && page.history_complete);
    assert!(page.requests.is_empty());
    let (v4_binary, v4_json) = super::super::legacy::encode_v4(&page.cursor);
    label["parentId"] = serde_json::json!("between-label");
    append(&input, &format!("{label}\n"));
    let mut later: Value = serde_json::from_str(&conflict).unwrap();
    later["id"] = serde_json::json!("later-user");
    later["parentId"] = label["id"].clone();
    append(&input, &format!("{later}\n"));
    for cursor in [
        restored(&page.cursor),
        NativeFileCursor::convert_v4(&v4_binary).unwrap(),
        NativeFileCursor::convert_v4_json(&v4_json).unwrap(),
    ] {
        let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        assert!(page.admission_failure.is_none() && !page.history_complete);
        let public = messages(&page.evidence.events).collect::<Vec<_>>();
        assert_eq!(public.len(), 1);
        assert_admitted(public[0]);
        assert!(page.evidence.events.iter().all(|event| event.live_eligible));
        assert!(
            matches!(public[0], NativeMessageObservation::User(value) if !value.context.complete_through_event)
        );
    }
}
