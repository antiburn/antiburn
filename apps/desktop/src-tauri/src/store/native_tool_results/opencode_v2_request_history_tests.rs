use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::native_tool_results::evidence::{
    EvidencePublicationPayload, NativeEvidencePayload, current_binding_in, message_payload,
};
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::domain::{
    ActivationId, Generation, Observation, ObservationKind, ObservationPosition,
};
use antiburn_checks::preparation::ConditionOutcome;
use antiburn_checks::selection::{SelectionOutcome, SelectionState};
use antiburn_checks::source::{PublicationCursor, PublishedEvidenceSource, SourceEvent};
use antiburn_local::analysis::opencode_v2_events::OpenCodeV2EventJoiner;
use antiburn_local::analysis::{NativeEvidenceFact, NativeMessageObservation};
use serde_json::Value;
use std::sync::Arc;

fn execution(index: u64, sequence: &mut u64, text: &str) -> Vec<Value> {
    let fixture: Vec<Value> = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/opencode_public_events/v2.jsonl"
    ))
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect();
    fixture[..14]
        .iter()
        .chain(fixture.last())
        .cloned()
        .map(|mut event| {
            *sequence += 1;
            event["id"] = json!(format!("{index}-{}", event["id"].as_str().unwrap()));
            event["durable"]["aggregateID"] = json!("native-session");
            event["durable"]["seq"] = json!(*sequence);
            event["data"]["sessionID"] = json!("native-session");
            for field in ["inboxID", "assistantMessageID", "id"] {
                if let Some(id) = event["data"][field].as_str() {
                    event["data"][field] = json!(format!("{index}-{id}"));
                }
            }
            if event["type"] == "session.inbox.enqueued" {
                event["data"]["item"]["payload"]["text"] = json!(text);
            }
            event
        })
        .collect()
}

fn publish(store: &Store, claim: &OpenCodeV2CaptureClaim, events: &[Value]) -> Vec<bool> {
    let mut joiner = OpenCodeV2EventJoiner::new();
    joiner.set_live_baseline(claim.baseline_sequence.unwrap());
    let mut cycle_completeness = Vec::new();
    for event in events {
        let batch = joiner.observe_evidence(event);
        assert!(joiner.gaps().is_empty());
        cycle_completeness.extend(batch.events.iter().filter_map(|event| match &event.fact {
            NativeEvidenceFact::ResponseCycle(cycle) => Some(cycle.context.complete_through_event),
            _ => None,
        }));
        assert!(
            store
                .advance_opencode_v2_evidence(
                    claim,
                    event["durable"]["seq"].as_u64().unwrap(),
                    &batch,
                    101,
                    60,
                )
                .unwrap()
        );
    }
    cycle_completeness
}

fn turns(store: &Store) -> Vec<Observation> {
    let page = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_project(store, "native", "/synthetic"),
        100,
    )
    .unwrap()
    .read_batch(PublicationCursor(0))
    .unwrap();
    assert!(!page.has_more);
    page.events
        .into_iter()
        .filter_map(|event| match event {
            SourceEvent::Observation {
                cursor,
                observation,
                ..
            } if matches!(observation.kind, ObservationKind::TurnEnd { .. }) => {
                Some(observation.commit(ObservationPosition(cursor.0)))
            }
            _ => None,
        })
        .collect()
}

fn activation(condition: &str) -> EffectiveActivation {
    let source = format!(
        "name: Request history\non:\n  turn_end:\nif: {condition}\nactions:\n  - type: status\n    input: {{result: failure}}\n"
    );
    EffectiveActivation {
        id: ActivationId {
            scope: Default::default(),
            definition_id: "request-history".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: antiburn_checks::discovery::DefinitionScope::Global,
        source_paths: Vec::new(),
        project_root: Some("/synthetic".into()),
        compiled: Arc::new(antiburn_checks::compile(&source).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    }
}

fn complete_capture(store: &Store, claim: &OpenCodeV2CaptureClaim, sequence: &mut u64) {
    *sequence += 1;
    assert!(
        store
            .advance_opencode_v2_capture(claim, *sequence, Some(&result(*sequence, true)), 101, 60)
            .unwrap()
    );
}

fn assert_capture_history(
    store: &Store,
    claim: &OpenCodeV2CaptureClaim,
    sequence: u64,
    history_gap: bool,
) {
    let saved = store
        .opencode_v2_capture_claims(std::slice::from_ref(&claim.key))
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].baseline_sequence, claim.baseline_sequence);
    assert_eq!(saved[0].last_sequence, Some(sequence));
    assert_eq!(saved[0].history_gap, history_gap);
    assert_eq!(saved[0].capture_epoch, claim.capture_epoch);
    assert_eq!(saved[0].claim_fence, claim.claim_fence);
}

fn assert_cycle_bindings(store: &Store, expected_history: &[bool]) {
    let publications = store.native_evidence_publications_after(0, 100).unwrap();
    let mut history = Vec::new();
    for publication in &publications {
        let Some(EvidencePublicationPayload::Native(NativeEvidencePayload::ResponseCycle(cycle))) =
            &publication.payload
        else {
            continue;
        };
        history.push(cycle.context.complete_through_event);
        assert_eq!(cycle.messages.len(), 4);
        assert_eq!(publication.dependencies.len(), cycle.messages.len());
        let mut tool_results = 0;
        for (member, binding) in cycle.messages.iter().zip(&publication.dependencies) {
            let member_payload = message_payload(member);
            assert_eq!(binding.identity, member_payload.identity().unwrap());
            assert_eq!(
                binding.revision,
                i64::try_from(member_payload.revision()).unwrap()
            );
            assert_eq!(
                binding.payload_sha256,
                crate::store::burn_checks::payload_sha256(&member_payload.json().unwrap())
            );
            assert_eq!(
                current_binding_in(&store.lock(), &publication.session, &binding.identity)
                    .unwrap()
                    .as_ref(),
                Some(binding)
            );
            if let NativeMessageObservation::ToolResult(result) = member {
                tool_results += 1;
                assert!(!result.context.as_ref().unwrap().complete_through_event);
            }
        }
        assert_eq!(tool_results, 1);
    }
    assert_eq!(history, expected_history);
}

#[test]
fn v2_second_turn_selects_the_retained_constraint_without_adding_old_members() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    let mut sequence = claim.baseline_sequence.unwrap();
    complete_capture(&store, &claim, &mut sequence);
    let mut events = execution(1, &mut sequence, "Do not change generated files.");
    events.extend(execution(2, &mut sequence, "Read the synthetic file."));
    assert_eq!(publish(&store, &claim, &events), [true, true]);
    let turns = turns(&store);
    assert_eq!(turns.len(), 2);
    let SelectionOutcome::Candidate(first) =
        SelectionState::default().apply(&activation("true"), &turns[0])
    else {
        panic!("the first turn must remain usable after the second request");
    };
    assert_eq!(first.context.value["session"]["requests_complete"], true);
    assert_eq!(
        first.context.value["session"]["requests"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        first.context.value["session"]["requests"][0]["text"],
        "Do not change generated files."
    );
    let active = activation(
        "session.requests.length == 2 && contains(session.requests.*.text, 'Do not change generated files.')",
    );
    let SelectionOutcome::Candidate(candidate) =
        SelectionState::default().apply(&active, &turns[1])
    else {
        panic!("the second turn must keep the earlier constraint");
    };
    let session = &candidate.context.value["session"];
    assert_eq!(session["requests_complete"], true);
    assert_eq!(
        session["requests"][0]["text"],
        "Do not change generated files."
    );
    assert_eq!(session["requests"][1]["text"], "Read the synthetic file.");
    let members = candidate.context.value["turn"]["messages"]
        .as_array()
        .unwrap();
    assert_eq!(members.len(), 4);
    assert!(members.iter().all(|message| {
        message["id"].as_str().unwrap().contains("2-")
            && !message["id"].as_str().unwrap().contains("1-assistant")
    }));
    assert!(
        !serde_json::to_string(members)
            .unwrap()
            .contains("Do not change generated files.")
    );
    assert_cycle_bindings(&store, &[true, true]);
    assert_capture_history(&store, &claim, sequence, false);
}

#[test]
fn v2_partial_turn_history_remains_usable_but_blocks_request_membership() {
    for limit in ["discarded-request", "capture-origin"] {
        let (_directory, store, _journal, start) = setup();
        let claim = started(&store, &start);
        let mut sequence = claim.baseline_sequence.unwrap();
        if limit == "discarded-request" {
            complete_capture(&store, &claim, &mut sequence);
        }
        let mut events = execution(1, &mut sequence, "DISCARDED_EARLIER_REQUEST");
        if limit == "discarded-request" {
            let mut partial = execution(9, &mut sequence, "DISCARDED_PARTIAL_REQUEST");
            partial[0]["data"]["item"]["payload"]["files"] = json!([{"path":"synthetic.png"}]);
            events.extend([partial[0].clone(), partial[2].clone()]);
        }
        events.extend(execution(2, &mut sequence, "Read the synthetic file."));
        assert_eq!(
            publish(&store, &claim, &events),
            if limit == "capture-origin" {
                [true, true]
            } else {
                [true, false]
            },
            "{limit}"
        );
        let turns = turns(&store);
        assert_eq!(turns.len(), 2, "{limit}");
        let usable = activation("true");
        let SelectionOutcome::Candidate(candidate) =
            SelectionState::default().apply(&usable, &turns[1])
        else {
            panic!("partial request history must not block unrelated turn checks: {limit}");
        };
        assert_eq!(
            candidate.context.value["session"]["requests_complete"], false,
            "{limit}"
        );
        let requests = &candidate.context.value["session"]["requests"];
        let members = candidate.context.value["turn"]["messages"]
            .as_array()
            .unwrap();
        assert_eq!(members.len(), 4, "{limit}");
        assert!(
            members
                .iter()
                .all(|member| member["id"].as_str().unwrap().contains("2-"))
        );
        if limit == "discarded-request" {
            assert_eq!(requests.as_array().unwrap().len(), 1);
            assert_eq!(requests[0]["text"], "Read the synthetic file.");
            assert!(
                !serde_json::to_string(requests)
                    .unwrap()
                    .contains("DISCARDED_")
            );
        } else {
            assert_eq!(requests.as_array().unwrap().len(), 2);
        }
        assert_eq!(
            SelectionState::default().apply(&activation("session.requests.length == 2"), &turns[1]),
            SelectionOutcome::ConditionFiltered(ConditionOutcome::Unavailable),
            "{limit}"
        );
        assert_cycle_bindings(
            &store,
            if limit == "capture-origin" {
                &[false, false]
            } else {
                &[true, false]
            },
        );
        assert_capture_history(&store, &claim, sequence, true);
    }
}

#[test]
fn v2_cycle_rejects_changed_member_content_and_revision_with_frozen_contexts() {
    for complete in [false, true] {
        for change in ["content", "revision"] {
            let (_directory, store, _journal, start) = setup();
            let claim = started(&store, &start);
            let mut sequence = claim.baseline_sequence.unwrap();
            if complete {
                complete_capture(&store, &claim, &mut sequence);
            }
            let events = execution(1, &mut sequence, "Keep the original request.");
            let mut joiner = OpenCodeV2EventJoiner::new();
            joiner.set_live_baseline(claim.baseline_sequence.unwrap());
            let mut changed = false;
            for event in events {
                let mut batch = joiner.observe_evidence(&event);
                assert!(joiner.gaps().is_empty());
                for fact in &mut batch.events {
                    if let NativeEvidenceFact::ResponseCycle(cycle) = &mut fact.fact {
                        let member = cycle
                            .messages
                            .iter_mut()
                            .find_map(|member| match member {
                                NativeMessageObservation::User(user) => Some(user),
                                _ => None,
                            })
                            .unwrap();
                        if change == "content" {
                            member.message.parts[0].text = "Changed request snapshot.".into();
                        } else {
                            member.revision += 1;
                        }
                        changed = true;
                    }
                }
                assert!(
                    store
                        .advance_opencode_v2_evidence(
                            &claim,
                            event["durable"]["seq"].as_u64().unwrap(),
                            &batch,
                            101,
                            60,
                        )
                        .unwrap()
                );
            }
            assert!(changed);
            assert!(turns(&store).is_empty(), "{complete}: {change}");
            assert_capture_history(&store, &claim, sequence, !complete);
            let users = store
                .native_evidence_publications_after(0, 100)
                .unwrap()
                .into_iter()
                .filter_map(|publication| match publication.payload {
                    Some(EvidencePublicationPayload::Native(
                        NativeEvidencePayload::UserMessage(user),
                    )) => Some(user),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(users.len(), 1);
            assert_eq!(users[0].message.parts[0].text, "Keep the original request.");
            assert_eq!(users[0].context.complete_through_event, complete);
        }
    }
}

#[test]
fn v2_duplicate_terminal_does_not_restore_partial_capture() {
    use antiburn_local::analysis::{
        NativeActivityObservation, NativeEvidenceBatch, NativeEvidenceEvent,
    };

    let (_directory, store, _journal, mut start) = setup();
    let claim = started(&store, &start);
    let mut sequence = claim.baseline_sequence.unwrap();
    complete_capture(&store, &claim, &mut sequence);
    let mut duplicate = result(sequence, true);
    store
        .mark_opencode_v2_capture_gap(std::slice::from_ref(&claim), "capture-lost", 102)
        .unwrap();
    start.baseline_sequence = Some(sequence);
    let next = store
        .start_opencode_v2_capture_epoch(&[start], "epoch-b", 103, 60)
        .unwrap()
        .unwrap()
        .remove(0);
    sequence += 1;
    duplicate.position = ToolResultPosition::OpenCodeV2Event {
        sequence,
        item_index: 0,
    };
    assert!(
        store
            .advance_opencode_v2_evidence(
                &next,
                sequence,
                &NativeEvidenceBatch {
                    source_format: SourceFormat::OpenCodeSqliteV2,
                    history_complete: true,
                    events: vec![
                        NativeEvidenceEvent {
                            live_eligible: true,
                            fact: NativeEvidenceFact::Message(
                                NativeMessageObservation::ToolResult(duplicate)
                            ),
                        },
                        NativeEvidenceEvent {
                            live_eligible: true,
                            fact: NativeEvidenceFact::Activity(NativeActivityObservation {
                                source_format: SourceFormat::OpenCodeSqliteV2,
                                session_id: next.key.session_id.clone(),
                                position: ToolResultPosition::OpenCodeV2Event {
                                    sequence,
                                    item_index: 0
                                },
                                token: "retained-terminal-receipt".into(),
                                native_at_ms: None,
                            }),
                        },
                    ],
                },
                104,
                60,
            )
            .unwrap()
    );
    assert_capture_history(&store, &next, sequence, true);
    assert_eq!(
        store.native_tool_results_current(&next.key).unwrap().len(),
        1
    );
    let published_results = store
        .native_evidence_publications_after(0, 100)
        .unwrap()
        .into_iter()
        .filter(|publication| {
            matches!(
                publication.payload,
                Some(EvidencePublicationPayload::Native(
                    NativeEvidencePayload::ToolResult(_)
                ))
            )
        })
        .count();
    assert_eq!(published_results, 1);
    assert!(turns(&store).is_empty());
}
