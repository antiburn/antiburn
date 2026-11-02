use super::*;
use crate::analysis::{ToolResultCompleteness, ToolResultPosition};

#[test]
fn codex_ordinal_regression_reports_a_typed_failure_after_public_pages_and_all_restores() {
    let directory = TempDir::new().unwrap();
    let [source, regression] = native_admission_fixtures::codex_ordinal_regression();
    let (header, rest) = source.split_once('\n').unwrap();
    let input = input(
        &directory,
        "codex",
        "facts",
        SourceFormat::CodexRolloutJsonl,
        &format!("{header}\n"),
    );
    let baseline = read_native_file_page(&input, None, &|| false).unwrap();
    append(&input, rest);
    let prior = read_native_file_page(&input, Some(&baseline.cursor), &|| false).unwrap();
    assert!(prior.history_complete && prior.admission_failure.is_none());
    assert!(
        prior
            .evidence
            .events
            .iter()
            .all(|event| event.live_eligible)
    );
    assert!(prior.evidence.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::ResponseCycle(cycle) if cycle.context.main_session && cycle.context.complete_through_event)));
    assert_eq!(prior.results.len(), 1);
    let result_identity = prior.results[0].observation.clone();
    let (v4_binary, v4_json) = super::super::legacy::encode_v4(&prior.cursor);
    append(&input, &regression);
    for cursor in [
        prior.cursor.clone(),
        restored(&prior.cursor),
        serde_json::from_slice(&serde_json::to_vec(&prior.cursor).unwrap()).unwrap(),
        NativeFileCursor::convert_v4(&v4_binary).unwrap(),
        NativeFileCursor::convert_v4_json(&v4_json).unwrap(),
    ] {
        assert_eq!(cursor.offset(), source.len() as u64);
        assert_eq!(cursor.next_record_index, 12);
        let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        assert!(!page.has_more && !page.history_complete);
        assert!(page.results.is_empty() && page.requests.is_empty());
        assert_eq!(
            page.cursor.offset(),
            (source.len() + regression.len()) as u64
        );
        let failure = page.admission_failure.as_ref().unwrap();
        assert_eq!(failure.reason, ToolResultCompleteness::UnsafeOrder);
        assert_eq!(
            failure.position,
            ToolResultPosition::FileBytes {
                record_start: source.len() as u64,
                record_end: (source.len() + regression.len()) as u64,
                item_index: u32::MAX
            }
        );
        assert_eq!(page.evidence.events.len(), 1);
        assert!(page.evidence.events[0].live_eligible);
        assert!(
            matches!(&page.evidence.events[0].fact, NativeEvidenceFact::AdmissionFailure(value) if value == failure)
        );
        let next = read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert_eq!(next.admission_failure.as_ref(), Some(failure));
        assert!(next.evidence.events.is_empty());
    }
    let full = identity_proofs::full_events("codex", &format!("{source}{regression}"));
    assert!(full.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::AdmissionFailure(failure) if failure.reason == ToolResultCompleteness::UnsafeOrder && failure.position == ToolResultPosition::Jsonl { record_index: 12, item_index: u32::MAX })));
    let result = full
        .iter()
        .find_map(|event| match &event.fact {
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => Some(value),
            _ => None,
        })
        .unwrap();
    assert_eq!(result.record_id, result_identity.record_id);
    assert_eq!(result.call_id, result_identity.call_id);
    assert_eq!(result.result_id, result_identity.result_id);
    assert_eq!(result.completeness, ToolResultCompleteness::UnsafeOrder);
    for event in &full {
        match &event.fact {
            NativeEvidenceFact::Message(message) => {
                assert_unsafe_message(message, ToolResultCompleteness::UnsafeOrder)
            }
            NativeEvidenceFact::ResponseCycle(cycle) => {
                assert!(!cycle.context.main_session && !cycle.context.complete_through_event);
                for message in &cycle.messages {
                    assert_unsafe_message(message, ToolResultCompleteness::UnsafeOrder);
                }
            }
            _ => {}
        }
    }
    let directory = TempDir::new().unwrap();
    let same_page_input = super::input(
        &directory,
        "codex",
        "facts",
        SourceFormat::CodexRolloutJsonl,
        &format!("{source}{regression}"),
    );
    let page = read_native_file_page(&same_page_input, None, &|| false).unwrap();
    assert_eq!(
        page.admission_failure.unwrap().reason,
        ToolResultCompleteness::UnsafeOrder
    );
    assert!(page.evidence.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::ResponseCycle(cycle) if !cycle.context.main_session && !cycle.context.complete_through_event)));
}

#[test]
fn missing_codex_ordinals_and_forward_gaps_remain_soft() {
    for ordinal in [Value::Null, serde_json::json!(13)] {
        let directory = TempDir::new().unwrap();
        let [source, regression] = native_admission_fixtures::codex_ordinal_regression();
        let input = input(
            &directory,
            "codex",
            "facts",
            SourceFormat::CodexRolloutJsonl,
            &source,
        );
        let prior = read_native_file_page(&input, None, &|| false).unwrap();
        let mut gap: Value = serde_json::from_str(&regression).unwrap();
        gap["ordinal"] = ordinal;
        append(&input, &format!("{gap}\n"));
        let page =
            read_native_file_page(&input, Some(&restored(&prior.cursor)), &|| false).unwrap();
        assert!(!page.history_complete && page.admission_failure.is_none());
        assert!(page.evidence.events.is_empty());
        let full = identity_proofs::full_events("codex", &format!("{source}{gap}\n"));
        assert!(
            !full
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
        );
        assert!(full.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::ResponseCycle(cycle) if cycle.context.main_session)));
        let mut later = gap.clone();
        later["ordinal"] = serde_json::json!(if gap["ordinal"].is_null() { 10 } else { 14 });
        append(&input, &format!("{later}\n"));
        let later_page =
            read_native_file_page(&input, Some(&restored(&page.cursor)), &|| false).unwrap();
        assert!(!later_page.history_complete && later_page.admission_failure.is_none());
        let full = identity_proofs::full_events("codex", &format!("{source}{gap}\n{later}\n"));
        assert!(
            !full
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
        );
    }
}

#[test]
fn codex_completed_message_cache_limits_do_not_prove_unsafe_order() {
    let source = include_str!("../../../../tests/fixtures/public_events/codex.jsonl");
    let mut source = source.to_owned();
    for index in 0..4096 {
        let record = serde_json::json!({"timestamp":"2026-10-07T10:01:00Z","ordinal":index + 12,"type":"response_item","payload":{"type":"message","id":format!("context-{index}"),"role":"developer","content":[{"type":"input_text","text":"Synthetic context."}]}});
        source.push_str(&format!("{record}\n"));
    }
    let events = identity_proofs::full_events("codex", &source);
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    assert!(events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::ResponseCycle(cycle) if cycle.context.main_session && cycle.context.complete_through_event)));
}

#[test]
fn codex_terminal_replay_has_no_public_work_or_idle_activity_after_current_cursor_restore() {
    let source = include_str!("../../../../tests/fixtures/public_events/codex.jsonl");
    for terminal_type in ["task_complete", "turn_complete"] {
        let directory = TempDir::new().unwrap();
        let input = input(
            &directory,
            "codex",
            "facts",
            SourceFormat::CodexRolloutJsonl,
            source,
        );
        let prior = read_native_file_page(&input, None, &|| false).unwrap();
        let mut terminal: Value = serde_json::from_str(source.lines().last().unwrap()).unwrap();
        terminal["ordinal"] = serde_json::json!(12);
        terminal["payload"]["type"] = serde_json::json!(terminal_type);
        append(&input, &format!("{terminal}\n"));
        for cursor in [
            prior.cursor.clone(),
            restored(&prior.cursor),
            serde_json::from_slice(&serde_json::to_vec(&prior.cursor).unwrap()).unwrap(),
        ] {
            let replay = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            assert!(replay.admission_failure.is_none() && replay.history_complete);
            assert!(replay.evidence.events.is_empty() && replay.results.is_empty());
            assert_eq!(
                serde_json::to_value(&replay.requests).unwrap(),
                serde_json::to_value(&prior.requests).unwrap()
            );
        }
        let replay =
            read_native_file_page(&input, Some(&restored(&prior.cursor)), &|| false).unwrap();
        terminal["ordinal"] = serde_json::json!(13);
        terminal["payload"]["turn_id"] = serde_json::json!("turn-2");
        append(&input, &format!("{terminal}\n"));
        let genuine =
            read_native_file_page(&input, Some(&restored(&replay.cursor)), &|| false).unwrap();
        assert_eq!(genuine.evidence.events.len(), 1);
        assert!(matches!(
            &genuine.evidence.events[0].fact,
            NativeEvidenceFact::Activity(_)
        ));
        assert!(genuine.evidence.events[0].live_eligible);
    }
}

#[test]
fn codex_corrected_terminal_replays_keep_original_proof_and_frozen_cycle_identity() {
    use crate::analysis::{PublicMessagePayload, ToolResultChange};
    for public_correction in [false, true] {
        let directory = TempDir::new().unwrap();
        let [source, mut correction] = native_admission_fixtures::codex_terminal_correction();
        let original_terminal: Value =
            serde_json::from_str(source.lines().last().unwrap()).unwrap();
        if !public_correction {
            let mut terminal: Value = serde_json::from_str(source.lines().last().unwrap()).unwrap();
            terminal["ordinal"] = serde_json::json!(12);
            terminal["payload"]["error"] =
                serde_json::json!({"codex_error_info":"server_overloaded"});
            correction = format!("{terminal}\n");
        }
        let input = input(
            &directory,
            "codex",
            "facts",
            SourceFormat::CodexRolloutJsonl,
            &source,
        );
        let original = read_native_file_page(&input, None, &|| false).unwrap();
        let original_cycle = original
            .evidence
            .events
            .iter()
            .find_map(|event| match &event.fact {
                NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle),
                _ => None,
            })
            .unwrap();
        append(&input, &correction);
        let corrected =
            read_native_file_page(&input, Some(&restored(&original.cursor)), &|| false).unwrap();
        assert!(corrected.admission_failure.is_none() && corrected.history_complete);
        let removals = corrected
            .evidence
            .events
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(removals.len(), 1);
        assert_eq!(removals[0].key, original_cycle.key);
        assert_eq!(removals[0].start_position, original_cycle.start_position);
        assert_eq!(
            removals[0].context.first_eligible_position,
            original_cycle.context.first_eligible_position
        );
        assert_eq!(removals[0].change, ToolResultChange::Removal);
        assert!(removals[0].messages.is_empty());
        assert_eq!(
            corrected
                .evidence
                .events
                .iter()
                .filter(|event| matches!(&event.fact, NativeEvidenceFact::Activity(_)))
                .count(),
            if public_correction { 3 } else { 1 }
        );
        if public_correction {
            assert!(corrected.evidence.events.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(message)) if message.record_id == "assistant-correction" && message.eligible && matches!(&message.payload, PublicMessagePayload::AssistantText { parts, .. } if parts[0].text == "The corrected review is complete."))));
        }
        let mut source = format!("{source}{correction}");
        let corrected_terminal: Value =
            serde_json::from_str(correction.lines().last().unwrap()).unwrap();
        let mut next_correction = corrected_terminal.clone();
        next_correction["payload"]["error"] = serde_json::json!({"codex_error_info":"rate_limit"});
        let mut cursor = corrected.cursor;
        for (offset, (terminal, alias, new_activity)) in [
            (&corrected_terminal, "task_complete", false),
            (&corrected_terminal, "turn_complete", false),
            (&original_terminal, "task_complete", false),
            (&original_terminal, "turn_complete", false),
            (&corrected_terminal, "turn_complete", false),
            (&next_correction, "task_complete", true),
            (&next_correction, "turn_complete", false),
            (&original_terminal, "turn_complete", false),
            (&next_correction, "task_complete", false),
        ]
        .into_iter()
        .enumerate()
        {
            let mut replay = terminal.clone();
            replay["ordinal"] = serde_json::json!(
                corrected_terminal["ordinal"].as_u64().unwrap() + offset as u64 + 1
            );
            replay["payload"]["type"] = serde_json::json!(alias);
            let replay = format!("{replay}\n");
            append(&input, &replay);
            let full_before = identity_proofs::full_events("codex", &source);
            source.push_str(&replay);
            for restored in [
                cursor.clone(),
                super::restored(&cursor),
                serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap(),
            ] {
                let page = read_native_file_page(&input, Some(&restored), &|| false).unwrap();
                assert!(page.admission_failure.is_none() && page.history_complete);
                assert!(page.results.is_empty());
                assert_eq!(page.evidence.events.len(), usize::from(new_activity));
                assert!(page.evidence.events.iter().all(|event| matches!(
                    event.fact,
                    NativeEvidenceFact::Activity(_)
                ) && event.live_eligible));
            }
            let full = identity_proofs::full_events("codex", &source);
            if new_activity {
                assert_eq!(&full[..full_before.len()], full_before);
                assert_eq!(full.len(), full_before.len() + 1);
                assert!(matches!(
                    &full.last().unwrap().fact,
                    NativeEvidenceFact::Activity(_)
                ));
            } else {
                assert_eq!(full, full_before);
            }
            cursor = read_native_file_page(&input, Some(&super::restored(&cursor)), &|| false)
                .unwrap()
                .cursor;
        }
    }
}
