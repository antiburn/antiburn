use super::*;
use antiburn_local::analysis::{
    OpenCodeJournalCursor, OpenCodeToolResultPage, PublicMessageObservation, ToolResultAdmission,
    ToolResultCompleteness, UserMessageObservation, read_opencode_live_tool_result_page as read,
};

fn round_trip(cursor: &OpenCodeJournalCursor) -> OpenCodeJournalCursor {
    serde_json::from_slice(&serde_json::to_vec(cursor).unwrap()).unwrap()
}

fn v4_cursor(cursor: &OpenCodeJournalCursor) -> OpenCodeJournalCursor {
    let mut legacy = serde_json::to_value(cursor).unwrap();
    legacy["parser_revision"] = json!(62);
    legacy["live"]["revision"] = json!(4);
    legacy["live"].as_object_mut().unwrap().remove("public");
    legacy["live"]
        .as_object_mut()
        .unwrap()
        .remove("public_baseline");
    legacy["live"]["requests"]
        .as_object_mut()
        .unwrap()
        .remove("removed_parts");
    serde_json::from_value(legacy).unwrap()
}

fn users(page: &OpenCodeToolResultPage) -> Vec<&UserMessageObservation> {
    page.evidence
        .events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::User(user)) => Some(user),
            _ => None,
        })
        .collect()
}

fn calls(page: &OpenCodeToolResultPage) -> Vec<&PublicMessageObservation> {
    page.evidence
        .events
        .iter()
        .filter_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::Public(public))
                if matches!(public.payload, PublicMessagePayload::ToolCall { .. }) =>
            {
                Some(public)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn parser_63_live_cursor_keeps_progress_public_baseline_and_frozen_context() {
    compatible_live_cursor_keeps_progress_public_baseline_and_frozen_context(63);
}

#[test]
fn parser_64_live_cursor_keeps_progress_public_baseline_and_frozen_context() {
    compatible_live_cursor_keeps_progress_public_baseline_and_frozen_context(64);
}

#[test]
fn parser_65_saved_live_cursor_keeps_progress_public_baseline_and_frozen_context() {
    compatible_live_cursor_keeps_progress_public_baseline_and_frozen_context(65);
}

#[test]
fn parser_66_saved_live_cursor_keeps_progress_public_baseline_and_frozen_context() {
    compatible_live_cursor_keeps_progress_public_baseline_and_frozen_context(66);
}

fn compatible_live_cursor_keeps_progress_public_baseline_and_frozen_context(parser_revision: i64) {
    use antiburn_local::analysis::{
        PARSER_REVISION, ToolResultStatus, opencode_journal_has_new_events,
    };
    let (directory, path, connection) = journal();
    v1_messages_with_tool_status(&connection, true);
    let initial = read(&path, "root", None, None, &|| false).unwrap();
    assert_eq!(initial.results.len(), 1);
    assert!(initial.results[0].finality);
    assert_eq!(initial.results[0].status, ToolResultStatus::Success);
    assert_eq!(
        initial.results[0].position,
        ToolResultPosition::OpenCodeEvent {
            sequence: 6,
            item_index: 0,
        }
    );
    assert_eq!(initial.cursor.public_baseline(), Some(8));
    let context = initial.results[0].context.as_ref().unwrap();
    assert_eq!(context.user_messages.len(), 1);
    assert_eq!(
        context.user_messages[0].parts[0].text,
        "Inspect the synthetic file"
    );
    let mut old = serde_json::to_value(&initial.cursor).unwrap();
    old["parser_revision"] = json!(parser_revision);
    assert_eq!(old["live"]["revision"], 5);
    assert!(old["live"]["requests"]["messages"]["user"].is_array());
    assert!(old["live"]["parts"]["tool"].is_object());
    assert!(!old["live"]["contexts"].as_array().unwrap().is_empty());
    let saved_path = directory.path().join("saved-live-cursor.json");
    std::fs::write(&saved_path, serde_json::to_vec(&old).unwrap()).unwrap();
    let cursor: OpenCodeJournalCursor =
        serde_json::from_slice(&std::fs::read(&saved_path).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&cursor).unwrap(), old);
    assert!(opencode_journal_has_new_events(&path, "root", &cursor).unwrap());
    let upgraded = read(&path, "root", Some(&cursor), None, &|| false).unwrap();
    assert!(upgraded.results.is_empty());
    assert!(upgraded.evidence.events.is_empty());
    let saved = serde_json::to_value(&upgraded.cursor).unwrap();
    assert_eq!(saved["parser_revision"], PARSER_REVISION);
    assert_eq!(saved["through_sequence"], old["through_sequence"]);
    assert_eq!(saved["live"], old["live"]);
    append(
        &connection,
        9,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"user-part","messageID":"user","sessionID":"root","type":"text","text":"Corrected input"
        }}),
    );
    append(
        &connection,
        10,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"tool","messageID":"assistant","sessionID":"root","type":"tool","callID":"call","tool":"read",
            "state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"Corrected contents","title":"Read synthetic.rs","metadata":{},"time":{"start":1002,"end":1003}}
        }}),
    );
    let later = read(
        &path,
        "root",
        Some(&round_trip(&upgraded.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    assert_eq!(later.results.len(), 1);
    assert_eq!(later.results[0].context, initial.results[0].context);
    assert_eq!(later.results[0].record_id, initial.results[0].record_id);
    assert_eq!(later.results[0].call_id, initial.results[0].call_id);
    assert_eq!(later.results[0].result_id, initial.results[0].result_id);
    assert!(later.results[0].revision > initial.results[0].revision);
    assert_eq!(
        later.results[0].content.as_deref(),
        Some("Corrected contents")
    );
    assert_eq!(
        later.results[0].position,
        ToolResultPosition::OpenCodeEvent {
            sequence: 10,
            item_index: 0,
        }
    );
    assert_eq!(
        later.results[0].context.as_ref().unwrap().result_position,
        initial.results[0].position
    );
    assert_eq!(
        later.cursor.public_baseline(),
        initial.cursor.public_baseline()
    );
    assert_eq!(
        serde_json::to_value(&later.cursor).unwrap()["through_sequence"],
        10
    );
    assert_eq!(users(&later).len(), 1);
    assert!(
        later
            .evidence
            .events
            .iter()
            .filter(|event| matches!(
                event.fact,
                NativeEvidenceFact::Message(NativeMessageObservation::User(_))
            ))
            .all(|event| !event.live_eligible)
    );
    assert_eq!(
        users(&later)[0].context.first_eligible_position,
        ToolResultPosition::OpenCodeEvent {
            sequence: 2,
            item_index: 0
        }
    );
}

#[test]
fn compatible_parser_revisions_require_the_current_journal_layout() {
    use antiburn_local::analysis::{OpenCodeJournalReadError, PARSER_REVISION};
    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let initial = read(&path, "root", None, None, &|| false).unwrap();
    for parser_revision in [63, 64, 65, 66, PARSER_REVISION] {
        for live_revision in [2, 4, 6] {
            let mut incompatible = serde_json::to_value(&initial.cursor).unwrap();
            incompatible["parser_revision"] = json!(parser_revision);
            incompatible["live"]["revision"] = json!(live_revision);
            let cursor = serde_json::from_value(incompatible).unwrap();
            let error = read(&path, "root", Some(&cursor), None, &|| false).unwrap_err();
            assert_eq!(
                error.downcast_ref::<OpenCodeJournalReadError>(),
                Some(&OpenCodeJournalReadError::SourceChanged)
            );
        }
        let mut missing_baseline = serde_json::to_value(&initial.cursor).unwrap();
        missing_baseline["parser_revision"] = json!(parser_revision);
        missing_baseline["live"]
            .as_object_mut()
            .unwrap()
            .remove("public_baseline");
        let cursor = serde_json::from_value(missing_baseline).unwrap();
        assert!(read(&path, "root", Some(&cursor), None, &|| false).is_err());
    }
}

#[test]
fn parser_66_cold_retry_keeps_its_original_digest_and_frontier() {
    use sha2::{Digest, Sha256};

    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let initial = read(&path, "root", None, None, &|| false).unwrap();
    let mut old = serde_json::to_value(&initial.cursor).unwrap();
    old["parser_revision"] = json!(66);
    let cursor: OpenCodeJournalCursor = serde_json::from_value(old).unwrap();
    let mut receipt: serde_json::Value =
        serde_json::from_str(&initial.cold_retry_json().unwrap().unwrap()).unwrap();
    let digest: [u8; 32] = Sha256::digest(
        serde_json::to_vec(&(
            &cursor,
            &initial.evidence,
            initial.history_complete,
            &receipt["cold_replay"]["assistant_projections"],
        ))
        .unwrap(),
    )
    .into();
    receipt["parser_revision"] = json!(66);
    receipt["cold_replay"]["output_sha256"] = json!(digest);
    let cursor: OpenCodeJournalCursor = serde_json::from_value(receipt.clone()).unwrap();
    let retry = read(&path, "root", Some(&cursor), None, &|| false).unwrap();
    assert_eq!(retry.cursor, initial.cursor);
    assert_eq!(retry.evidence, initial.evidence);
    assert_eq!(retry.results, initial.results);

    receipt["cold_replay"]["output_sha256"][0] = json!(digest[0] ^ 1);
    let invalid = serde_json::from_value(receipt).unwrap();
    assert!(read(&path, "root", Some(&invalid), None, &|| false).is_err());
}

#[test]
fn parser_63_strict_cursor_continues_after_its_existing_frontier() {
    compatible_strict_cursor_continues_after_its_existing_frontier(63);
}

#[test]
fn parser_64_strict_cursor_continues_after_its_existing_frontier() {
    compatible_strict_cursor_continues_after_its_existing_frontier(64);
}

#[test]
fn parser_65_strict_cursor_continues_after_its_existing_frontier() {
    compatible_strict_cursor_continues_after_its_existing_frontier(65);
}

#[test]
fn parser_66_strict_cursor_continues_after_its_existing_frontier() {
    compatible_strict_cursor_continues_after_its_existing_frontier(66);
}

fn compatible_strict_cursor_continues_after_its_existing_frontier(parser_revision: i64) {
    use antiburn_local::analysis::{PARSER_REVISION, read_opencode_tool_result_page};
    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let initial = read_opencode_tool_result_page(&path, "root", None, &|| false).unwrap();
    let mut old = serde_json::to_value(&initial.cursor).unwrap();
    old["parser_revision"] = json!(parser_revision);
    let cursor = serde_json::from_value(old.clone()).unwrap();
    let idle = read_opencode_tool_result_page(&path, "root", Some(&cursor), &|| false).unwrap();
    assert!(idle.results.is_empty());
    let saved = serde_json::to_value(&idle.cursor).unwrap();
    assert_eq!(saved["parser_revision"], PARSER_REVISION);
    assert_eq!(saved["through_sequence"], old["through_sequence"]);
}

#[test]
fn idle_v4_conversion_retains_user_identity_after_synthetic_or_ignored_replacement() {
    for marker in ["synthetic", "ignored"] {
        let (_directory, path, connection) = journal();
        v1_messages(&connection);
        let mut replacement = json!({"id":"user-part","sessionID":"root","messageID":"user","type":"text","text":"Generated replacement"});
        replacement[marker] = json!(true);
        append(
            &connection,
            8,
            "message.part.updated.1",
            json!({"sessionID":"root","part":replacement}),
        );
        let consumed = read(&path, "root", None, None, &|| false).unwrap();
        let legacy = v4_cursor(&consumed.cursor);
        let before = serde_json::to_value(&legacy).unwrap();
        let converted = read(&path, "root", Some(&legacy), None, &|| false).unwrap();
        assert!(converted.evidence.events.is_empty());
        assert_eq!(converted.cursor.public_baseline(), Some(8));
        let converted_cursor = round_trip(&converted.cursor);
        let saved = serde_json::to_value(&converted_cursor).unwrap();
        assert_eq!(saved["through_sequence"], 8);
        assert_eq!(saved["live"]["baseline"], before["live"]["baseline"]);
        assert_eq!(saved["live"]["parts"], before["live"]["parts"]);
        assert_eq!(saved["live"]["contexts"], before["live"]["contexts"]);
        assert_eq!(
            saved["live"]["requests"]["messages"],
            before["live"]["requests"]["messages"]
        );
        assert_eq!(
            saved["live"]["requests"]["parts"],
            before["live"]["requests"]["parts"]
        );
        assert!(
            saved["live"]["public"]["historical_users"]
                .get("user")
                .is_some()
        );
        append(
            &connection,
            9,
            "message.part.updated.1",
            json!({"sessionID":"root","part":{
                "id":"user-part","sessionID":"root","messageID":"user","type":"text","text":"Corrected real input"
            }}),
        );
        append(
            &connection,
            10,
            "message.updated.1",
            json!({"sessionID":"root","info":{
                "id":"new-user","sessionID":"root","role":"user"
            }}),
        );
        append(
            &connection,
            11,
            "message.part.updated.1",
            json!({"sessionID":"root","part":{
                "id":"new-user-part","sessionID":"root","messageID":"new-user","type":"text","text":"Genuine new input"
            }}),
        );
        append(
            &connection,
            12,
            "message.part.updated.1",
            json!({"sessionID":"root","part":{
                "id":"tool","sessionID":"root","messageID":"assistant","type":"tool","callID":"call","tool":"read",
                "state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"contents"}
            }}),
        );
        let corrected = read(&path, "root", Some(&converted_cursor), None, &|| false).unwrap();
        assert_eq!(users(&corrected).len(), 2);
        for event in &corrected.evidence.events {
            if let NativeEvidenceFact::Message(NativeMessageObservation::User(user)) = &event.fact {
                if user.message.message_id == "user" {
                    assert!(!event.live_eligible, "{marker}");
                    assert_eq!(user.message.parts[0].text, "Corrected real input");
                    assert!(matches!(user.context.first_eligible_position,
                        ToolResultPosition::OpenCodeEvent { sequence, .. } if sequence <= 8));
                } else {
                    assert_eq!(user.message.message_id, "new-user");
                    assert!(event.live_eligible, "{marker}");
                }
            }
        }
        assert_eq!(corrected.results.len(), 1);
        assert_eq!(
            corrected.results[0].admission(),
            ToolResultAdmission::Admitted
        );
        assert!(
            read(
                &path,
                "root",
                Some(&round_trip(&corrected.cursor)),
                None,
                &|| false
            )
            .unwrap()
            .results
            .is_empty()
        );
    }
}

#[test]
fn unread_historical_user_without_accepted_text_stays_context_only() {
    for generated_part in [false, true] {
        let (_directory, path, connection) = journal();
        let initial = read(&path, "root", None, None, &|| false).unwrap();
        let legacy = v4_cursor(&initial.cursor);
        append(
            &connection,
            1,
            "message.updated.1",
            json!({"sessionID":"root","info":{
                "id":"user","sessionID":"root","role":"user"
            }}),
        );
        if generated_part {
            append(
                &connection,
                2,
                "message.part.updated.1",
                json!({"sessionID":"root","part":{
                    "id":"user-part","sessionID":"root","messageID":"user","type":"text","text":"Generated input","synthetic":true
                }}),
            );
        }
        let baseline = if generated_part { 2 } else { 1 };
        let converted = read(&path, "root", Some(&legacy), None, &|| false).unwrap();
        assert_eq!(converted.cursor.public_baseline(), Some(baseline));
        append(
            &connection,
            baseline + 1,
            "message.part.updated.1",
            json!({"sessionID":"root","part":{
                "id":"user-part","sessionID":"root","messageID":"user","type":"text","text":"First retained real input"
            }}),
        );
        let corrected = read(
            &path,
            "root",
            Some(&round_trip(&converted.cursor)),
            None,
            &|| false,
        )
        .unwrap();
        assert_eq!(users(&corrected).len(), 1);
        assert!(
            corrected
                .evidence
                .events
                .iter()
                .filter(|event| matches!(
                    event.fact,
                    NativeEvidenceFact::Message(NativeMessageObservation::User(_))
                ))
                .all(|event| !event.live_eligible)
        );
    }
}

#[test]
fn idle_v4_conversion_keeps_old_user_eligibility_without_replaying_results() {
    let (_directory, path, connection) = journal();
    v1_messages(&connection);
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    let mut legacy = serde_json::to_value(&seed.cursor).unwrap();
    legacy["parser_revision"] = json!(62);
    legacy["live"]["revision"] = json!(4);
    legacy["live"].as_object_mut().unwrap().remove("public");
    legacy["live"]
        .as_object_mut()
        .unwrap()
        .remove("public_baseline");
    legacy["live"]["requests"]
        .as_object_mut()
        .unwrap()
        .remove("removed_parts");
    let cursor: OpenCodeJournalCursor = serde_json::from_value(legacy.clone()).unwrap();
    let converted = read(&path, "root", Some(&cursor), None, &|| false).unwrap();
    assert!(converted.evidence.events.is_empty());
    let saved = serde_json::to_value(&converted.cursor).unwrap();
    assert_eq!(saved["through_sequence"], legacy["through_sequence"]);
    assert_eq!(saved["live"]["baseline"], legacy["live"]["baseline"]);
    assert_eq!(saved["live"]["parts"], legacy["live"]["parts"]);
    assert_eq!(saved["live"]["contexts"], legacy["live"]["contexts"]);
    assert_eq!(converted.cursor.public_baseline(), Some(7));
    append(
        &connection,
        8,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"user-part","messageID":"user","sessionID":"root","type":"text","text":"Corrected old input"
        }}),
    );
    append(
        &connection,
        9,
        "message.updated.1",
        json!({"sessionID":"root","info":{
            "id":"new-user","sessionID":"root","role":"user"
        }}),
    );
    append(
        &connection,
        10,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"new-user-part","messageID":"new-user","sessionID":"root","type":"text","text":"New input"
        }}),
    );
    append(
        &connection,
        11,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"tool","messageID":"assistant","sessionID":"root","type":"tool","callID":"call","tool":"read",
            "state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"contents"}
        }}),
    );
    let page = read(
        &path,
        "root",
        Some(&round_trip(&converted.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    for event in &page.evidence.events {
        if let NativeEvidenceFact::Message(NativeMessageObservation::User(user)) = &event.fact {
            match user.message.message_id.as_str() {
                "user" => {
                    assert!(!event.live_eligible);
                    assert_eq!(
                        user.context.first_eligible_position,
                        ToolResultPosition::OpenCodeEvent {
                            sequence: 2,
                            item_index: 0
                        }
                    );
                }
                "new-user" => assert!(event.live_eligible),
                other => panic!("unexpected user {other}"),
            }
        }
    }
    assert_eq!(users(&page).len(), 2);
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].admission(), ToolResultAdmission::Admitted);
    assert!(
        read(&path, "root", Some(&page.cursor), None, &|| false)
            .unwrap()
            .results
            .is_empty()
    );
}

#[test]
fn removed_user_part_cannot_return_to_public_text_or_later_result_prefixes() {
    let (_directory, path, connection) = journal();
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    v1_messages(&connection);
    append(
        &connection,
        8,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"user-part-b","messageID":"user","sessionID":"root","type":"text","text":"Retained input B"
        }}),
    );
    let initial = read(&path, "root", Some(&seed.cursor), None, &|| false).unwrap();
    assert_eq!(users(&initial).last().unwrap().message.parts.len(), 2);
    append(
        &connection,
        9,
        "message.part.removed.1",
        json!({"sessionID":"root","messageID":"user","partID":"user-part"}),
    );
    let removed = read(
        &path,
        "root",
        Some(&round_trip(&initial.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    assert!(removed.history_complete);
    assert!(removed.evidence.history_complete);
    assert_eq!(
        users(&removed)[0].message.change,
        UserMessageChange::Present
    );
    assert_eq!(users(&removed)[0].message.parts.len(), 1);
    assert_eq!(users(&removed)[0].message.parts[0].text, "Retained input B");
    let saved = serde_json::to_value(&removed.cursor).unwrap();
    assert_eq!(
        saved["live"]["requests"]["removed_parts"]["user-part"]["owner"],
        "user"
    );
    assert!(
        saved["live"]["requests"]["parts"]
            .get("user-part")
            .is_none()
    );
    append(
        &connection,
        10,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"user-part","messageID":"user","sessionID":"root","type":"text","text":"DELETED_INPUT_MUST_NOT_RETURN"
        }}),
    );
    append(
        &connection,
        11,
        "message.part.updated.1",
        json!({"sessionID":"root","part":{
            "id":"tool","messageID":"assistant","sessionID":"root","type":"tool","callID":"call","tool":"read",
            "state":{"status":"completed","input":{"path":"synthetic.rs"},"output":"contents"}
        }}),
    );
    let later = read(
        &path,
        "root",
        Some(&round_trip(&removed.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    assert!(later.history_complete);
    assert!(later.evidence.history_complete);
    assert!(users(&later).is_empty());
    assert_eq!(later.results.len(), 1);
    assert_eq!(later.results[0].admission(), ToolResultAdmission::Admitted);
    let user = later.results[0]
        .context
        .as_ref()
        .unwrap()
        .user_messages
        .iter()
        .find(|user| user.message_id == "user")
        .unwrap();
    assert_eq!(user.parts.len(), 1);
    assert_eq!(user.parts[0].native_part_id.as_deref(), Some("user-part-b"));
    assert!(
        !serde_json::to_string(&later.evidence)
            .unwrap()
            .contains("DELETED_INPUT_MUST_NOT_RETURN")
    );
    assert!(
        !serde_json::to_string(&later.cursor)
            .unwrap()
            .contains("DELETED_INPUT_MUST_NOT_RETURN")
    );
}

#[test]
fn every_tool_status_checks_the_original_public_call_binding() {
    for status in ["pending", "running", "completed", "error"] {
        for field in ["callID", "tool"] {
            let (_directory, path, connection) = journal();
            let seed = read(&path, "root", None, None, &|| false).unwrap();
            v1_messages(&connection);
            let initial = read(&path, "root", Some(&seed.cursor), None, &|| false).unwrap();
            let original = calls(&initial)[0].clone();
            let mut part = json!({"id":"tool","messageID":"assistant","sessionID":"root","type":"tool","callID":"call","tool":"read",
                "state":{"status":status,"input":{"path":"synthetic.rs"}}});
            match status {
                "pending" => part["state"]["raw"] = json!(""),
                "running" => part["state"]["time"] = json!({"start":1002}),
                "completed" => {
                    part["state"]["output"] = json!("contents");
                    part["state"]["title"] = json!("Synthetic read");
                    part["state"]["metadata"] = json!({});
                    part["state"]["time"] = json!({"start":1002,"end":1006});
                }
                "error" => {
                    part["state"]["error"] = json!("Synthetic failure");
                    part["state"]["time"] = json!({"start":1002,"end":1006});
                }
                _ => unreachable!(),
            }
            part[field] = json!(if field == "callID" {
                "replacement-call"
            } else {
                "write"
            });
            append(
                &connection,
                8,
                "message.part.updated.1",
                json!({"sessionID":"root","part":part}),
            );
            let changed = read(
                &path,
                "root",
                Some(&round_trip(&initial.cursor)),
                None,
                &|| false,
            )
            .unwrap();
            let withdrawn = calls(&changed);
            assert_eq!(withdrawn.len(), 1, "{status}, {field}");
            assert!(!withdrawn[0].eligible);
            assert_eq!(
                withdrawn[0].completeness,
                ToolResultCompleteness::UnsafeIdentity
            );
            assert_eq!(withdrawn[0].record_id, original.record_id);
            assert_eq!(withdrawn[0].call_id, original.call_id);
            assert_eq!(withdrawn[0].part_id, original.part_id);
            assert_eq!(withdrawn[0].payload, original.payload);
            assert_eq!(withdrawn[0].context, original.context);
            assert_eq!(
                withdrawn[0].position,
                ToolResultPosition::OpenCodeEvent {
                    sequence: 8,
                    item_index: 0
                }
            );
            assert!(
                changed
                    .results
                    .iter()
                    .all(|result| result.completeness == ToolResultCompleteness::UnsafeIdentity)
            );
            append(
                &connection,
                9,
                "message.part.updated.1",
                json!({"sessionID":"root","part":{
                    "id":"tool","messageID":"assistant","sessionID":"root","type":"tool","callID":"call","tool":"read",
                    "state":{"status":"running","input":{"path":"synthetic.rs"}}
                }}),
            );
            let restored = read(
                &path,
                "root",
                Some(&round_trip(&changed.cursor)),
                None,
                &|| false,
            )
            .unwrap();
            assert!(calls(&restored).is_empty());
        }
    }
}

#[test]
fn orphan_v2_stream_reasoning_and_tool_input_work_withhold_only_the_cycle() {
    for kind in [
        "session.step.streamed",
        "session.reasoning.started",
        "session.reasoning.ended",
        "session.tool.input.started",
        "session.tool.input.ended",
    ] {
        for ownership in ["missing", "seeded", "unfinished"] {
            let mut events = v2_fixture();
            let mut orphan = events
                .iter()
                .find(|event| event["id"] == "step-end-2")
                .unwrap()
                .clone();
            orphan["id"] = json!("orphan-work");
            orphan["type"] = json!(kind);
            orphan["data"] = json!({"sessionID":"root","assistantMessageID":"assistant-2"});
            match kind {
                "session.step.streamed" => {}
                "session.reasoning.started" => orphan["data"]["ordinal"] = json!(0),
                "session.reasoning.ended" => {
                    orphan["data"]["ordinal"] = json!(0);
                    orphan["data"]["text"] = json!("PRIVATE_ORPHAN_REASONING");
                }
                "session.tool.input.started" => {
                    orphan["data"]["id"] = json!("orphan-call");
                    orphan["data"]["name"] = json!("read");
                }
                "session.tool.input.ended" => {
                    orphan["data"]["id"] = json!("orphan-call");
                    orphan["data"]["text"] = json!("{}");
                }
                _ => unreachable!(),
            }
            let mut seeded = events
                .iter()
                .find(|event| event["id"] == "step-2")
                .unwrap()
                .clone();
            seeded["id"] = json!("seeded-step");
            seeded["durable"]["seq"] = json!(0);
            if ownership != "unfinished" {
                events.retain(|event| event["id"] != "step-2");
            }
            events.retain(|event| event["id"] != "step-end-2");
            events.insert(events.len() - 1, orphan);
            if ownership == "seeded" {
                events.insert(0, seeded);
            }
            let (facts, _) = v2_facts(&events, Some(0));
            assert!(
                !facts
                    .iter()
                    .any(|fact| matches!(fact, NativeEvidenceFact::ResponseCycle(_))),
                "{kind}, {ownership}"
            );
            assert!(
                facts.iter().any(|fact| matches!(fact,
                    NativeEvidenceFact::Message(NativeMessageObservation::Public(public))
                        if public.record_id == "assistant-1" && public.eligible
                            && matches!(public.payload, PublicMessagePayload::AssistantText { .. })
                )),
                "{kind}, {ownership}"
            );
            assert_eq!(
                facts
                    .iter()
                    .filter(|fact| matches!(
                        fact,
                        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
                    ))
                    .count(),
                1
            );
            assert!(
                !serde_json::to_string(&facts)
                    .unwrap()
                    .contains("PRIVATE_ORPHAN_REASONING")
            );
        }
    }
}

#[test]
fn known_assistant_text_removal_keeps_history_complete_but_unbound_removal_does_not() {
    let (_directory, path, connection) = journal();
    let seed = read(&path, "root", None, None, &|| false).unwrap();
    v1_messages(&connection);
    let initial = read(&path, "root", Some(&seed.cursor), None, &|| false).unwrap();
    append(
        &connection,
        8,
        "message.part.removed.1",
        json!({"sessionID":"root","messageID":"assistant","partID":"text"}),
    );
    let removed = read(
        &path,
        "root",
        Some(&round_trip(&initial.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    assert!(removed.history_complete);
    assert!(removed.evidence.history_complete);
    assert!(removed.evidence.events.iter().any(|event| matches!(&event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::Public(public))
            if !public.eligible && matches!(public.payload, PublicMessagePayload::AssistantText { .. })
    )));
    append(
        &connection,
        9,
        "message.part.removed.1",
        json!({"sessionID":"root","messageID":"assistant","partID":"text"}),
    );
    let repeated = read(
        &path,
        "root",
        Some(&round_trip(&removed.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    assert!(repeated.history_complete);
    assert!(
        read(
            &path,
            "root",
            Some(&round_trip(&repeated.cursor)),
            None,
            &|| false
        )
        .unwrap()
        .history_complete
    );
    append(
        &connection,
        10,
        "message.part.removed.1",
        json!({"sessionID":"root","messageID":"assistant","partID":"unbound-tool-part"}),
    );
    let unknown = read(
        &path,
        "root",
        Some(&round_trip(&repeated.cursor)),
        None,
        &|| false,
    )
    .unwrap();
    assert!(!unknown.history_complete);
    assert!(
        !read(
            &path,
            "root",
            Some(&round_trip(&unknown.cursor)),
            None,
            &|| false
        )
        .unwrap()
        .history_complete
    );
}
