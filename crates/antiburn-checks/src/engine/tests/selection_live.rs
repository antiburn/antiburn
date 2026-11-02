use super::*;
use crate::compiler::{RollingWindow, ToolCategory};
use crate::source::{
    PublicationRead, PublishedContext, PublishedEvidenceBatch, PublishedEvidenceChange,
    PublishedEvidenceEvent, PublishedEvidenceReader, PublishedEvidenceSource,
    PublishedResultStatus, PublishedToolResult, SourceError,
};

struct JournalReader(Vec<PublishedEvidenceEvent>);

impl PublishedEvidenceReader for JournalReader {
    fn read_batch(&self, request: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        let mut remaining = self.0.iter().filter(|event| event.cursor > request.after);
        let events: Vec<_> = remaining
            .by_ref()
            .take(usize::from(request.limit))
            .cloned()
            .collect();
        Ok(PublishedEvidenceBatch {
            next: events.last().map_or(request.after, |event| event.cursor),
            events,
            has_more: remaining.next().is_some(),
        })
    }
}

fn publication(
    cursor: u64,
    identity: &str,
    revision: u64,
    terminal: Option<u64>,
) -> PublishedEvidenceEvent {
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(cursor),
        branch: "branch-a".into(),
        context: Some(PublishedContext {
            session_selection: None,
            requests: vec![("request".into(), json!({"text": "Inspect the failure."}))],
            session_id: "session-a".into(),
            agent_name: "opencode".into(),
            project_root: None,
            version: format!("context-{}", terminal.unwrap_or(cursor)),
            main_session: true,
            complete_through_event: terminal.is_some(),
            inventories: crate::context::CurrentInventories::default(),
        }),
        change: PublishedEvidenceChange::Upsert(PublishedToolResult {
            identity: identity.into(),
            native_order: NativeOrderKey(format!("native-{}", terminal.unwrap_or(cursor))),
            first_terminal_publication: terminal.map(PublicationCursor),
            revision,
            tool_name: Some("bash".into()),
            category: Some("shell".into()),
            metadata: Box::default(),
            arguments: Some(json!({"command": "cargo check"})),
            content: terminal.map(|_| format!("{identity} output revision {revision}")),
            outcome: None,
            provider_executed: None,
            status: PublishedResultStatus::Error,
            finality: terminal.is_some(),
            payload_complete: terminal.is_some(),
            validity: crate::source::EvidenceValidity::Valid,
        }),
    }
}

fn public_message_publication(cursor: u64, kind: &str) -> PublishedEvidenceEvent {
    use crate::source::{
        PublishedObservation, PublishedObservationPayload, normalize_assistant_message,
        normalize_tool_call, normalize_user_message,
    };

    let identity = format!("{kind}-{cursor}");
    let mut event = publication(cursor, &identity, 1, Some(cursor));
    event.context.as_mut().unwrap().requests[0].1["source"] = json!("opencode");
    if kind == "tool_result" {
        return event;
    }
    let payload = match kind {
        "user_message" => PublishedObservationPayload::UserMessage(
            normalize_user_message(&identity, "opencode", "Public input", true).unwrap(),
        ),
        "assistant_message" => PublishedObservationPayload::AssistantMessage(
            normalize_assistant_message(
                &identity,
                "opencode",
                "Public output",
                true,
                &Default::default(),
            )
            .unwrap(),
        ),
        "tool_call" => PublishedObservationPayload::ToolCall(
            normalize_tool_call(
                &identity,
                "opencode",
                &format!("call-{cursor}"),
                "shell",
                Some(json!("explicit input")),
                &Default::default(),
                None,
                true,
            )
            .unwrap(),
        ),
        _ => panic!("unsupported fixture kind"),
    };
    event.change = PublishedEvidenceChange::ObservationUpsert(PublishedObservation {
        identity,
        native_order: NativeOrderKey(format!("native-{cursor}")),
        first_eligible_publication: Some(PublicationCursor(cursor)),
        revision: 1,
        eligible: true,
        payload_complete: true,
        validity: crate::source::EvidenceValidity::Valid,
        payload,
    });
    event
}

#[test]
fn explicit_unsafe_publications_clear_mixed_public_work_but_soft_gaps_keep_it_usable() {
    let kinds = [
        "user_message",
        "assistant_message",
        "tool_call",
        "tool_result",
    ];
    for unsafe_update in [false, true] {
        for split_page in [false, true] {
            let mut activation = window_activation();
            activation.compiled = Arc::new(crate::compile("name: Mixed public messages\non:\n  user_message:\n  assistant_message:\n  tool_call:\n  tool_result:\n  window: {size: 4, min: 4}\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap());
            let mut events = kinds
                .iter()
                .enumerate()
                .map(|(index, kind)| public_message_publication(101 + index as u64, kind))
                .collect::<Vec<_>>();
            events.push(PublishedEvidenceEvent {
                cursor: PublicationCursor(105),
                source_evidence_version: None,
                branch: "branch-a".into(),
                context: None,
                change: if unsafe_update {
                    PublishedEvidenceChange::UnsafeUpdate {
                        reason: "unsafe-identity".into(),
                    }
                } else {
                    PublishedEvidenceChange::PartialGap {
                        reason: "history-incomplete".into(),
                    }
                },
            });
            let source =
                PublishedEvidenceSource::new(JournalReader(events), if split_page { 4 } else { 5 })
                    .unwrap();
            let store = Arc::new(FakeStore::default());
            let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
            let first_page = source.read_batch(PublicationCursor(0)).unwrap();
            let initial = engine.apply_source_page(&activation, &first_page).unwrap();
            if split_page {
                assert_eq!(initial.candidates, 1);
                assert!(!engine.state.lock().unwrap().pending.is_empty());
                let marker = source.read_batch(first_page.next).unwrap();
                assert_eq!(
                    matches!(marker.events[0], SourceEvent::UnsafeUpdate { .. }),
                    unsafe_update
                );
                let applied = engine.apply_source_page(&activation, &marker).unwrap();
                assert_eq!(applied.observations, 0);
                assert_eq!(applied.candidates, 0);
            } else {
                assert_eq!(initial.candidates, usize::from(!unsafe_update));
            }
            assert_eq!(
                engine.state.lock().unwrap().pending.is_empty(),
                unsafe_update
            );
            assert_eq!(store.pending.lock().unwrap().is_empty(), unsafe_update);
            assert_eq!(store.committed.lock().unwrap().len(), 4);
            {
                let progress = store.progress.lock().unwrap();
                assert_eq!(progress.publication_cursor, PublicationCursor(105));
                assert_eq!(progress.observation_position, ObservationPosition(4));
                let branch = &progress.selection.branches[&BranchId("branch-a".into())];
                assert!(!branch.coverage_complete);
                assert_eq!(branch.results.len(), if unsafe_update { 0 } else { 4 });
                for (index, kind) in kinds.iter().enumerate() {
                    let identity = format!("{kind}-{}", 101 + index);
                    assert_eq!(
                        progress.selection.seen_results[&(BranchId("branch-a".into()), identity)]
                            .membership,
                        if unsafe_update {
                            crate::selection::ResultMembership::Retired
                        } else {
                            crate::selection::ResultMembership::Selected
                        }
                    );
                }
            }
            let earlier_candidates = store.stored_candidates.lock().unwrap().len();
            let continuation = PublishedEvidenceSource::new(
                JournalReader(
                    kinds
                        .iter()
                        .enumerate()
                        .map(|(index, kind)| public_message_publication(106 + index as u64, kind))
                        .collect(),
                ),
                4,
            )
            .unwrap()
            .read_batch(PublicationCursor(105))
            .unwrap();
            let next = engine
                .apply_source_page(&activation, &continuation)
                .unwrap();
            assert_eq!(next.candidates, if unsafe_update { 1 } else { 4 });
            let stored = store.stored_candidates.lock().unwrap();
            let new_candidates = &stored[earlier_candidates..];
            assert_eq!(
                new_candidates
                    .last()
                    .unwrap()
                    .messages
                    .iter()
                    .map(|message| message.id.as_str())
                    .collect::<Vec<_>>(),
                [
                    "user_message-106",
                    "assistant_message-107",
                    "tool_call-108",
                    "tool_result-109"
                ]
            );
            assert_eq!(
                new_candidates.last().unwrap().context.value["session"]["source_coverage"],
                "partial"
            );
            if !unsafe_update {
                assert_eq!(
                    new_candidates[0]
                        .messages
                        .iter()
                        .map(|message| message.id.as_str())
                        .collect::<Vec<_>>(),
                    [
                        "assistant_message-102",
                        "tool_call-103",
                        "tool_result-104",
                        "user_message-106"
                    ]
                );
            }
        }
    }
}

fn page(events: Vec<SourceEvent>) -> SourceBatch {
    let next = match events.last().expect("test page is not empty") {
        SourceEvent::Observation { cursor, .. } | SourceEvent::Removal { cursor, .. } => *cursor,
        _ => panic!("test page ends in an observation or removal"),
    };
    SourceBatch {
        events,
        next,
        has_more: false,
    }
}

fn native_member_publication(
    cursor: u64,
    revision: u64,
    first_terminal: u64,
) -> PublishedEvidenceEvent {
    let mut event = publication(cursor, "member", revision, Some(first_terminal));
    event.source_evidence_version = Some(EvidenceVersion(format!(
        "native-member-revision-{revision}"
    )));
    event
}

fn cycle_publication(cursor: u64, member: &PublishedEvidenceEvent) -> PublishedEvidenceEvent {
    use crate::source::{
        PublishedObservation, PublishedObservationPayload, PublishedResponseCycle,
        PublishedSourceEvidence, ResponseCycleStatus,
    };

    let PublishedEvidenceChange::Upsert(result) = &member.change else {
        unreachable!()
    };
    let member_message =
        crate::source::normalize_tool_result_message(result, member.context.as_ref().unwrap())
            .unwrap();
    let normalized_member = PublishedEvidenceSource::new(JournalReader(vec![member.clone()]), 10)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let SourceEvent::Observation {
        observation: member_observation,
        source_evidence_version,
        ..
    } = &normalized_member.events[0]
    else {
        unreachable!()
    };
    let member_native_version = member
        .source_evidence_version
        .clone()
        .expect("a cycle member has an exact native-current token");
    assert_eq!(
        source_evidence_version.as_ref(),
        Some(&member_native_version)
    );
    assert_ne!(member_native_version, member_observation.version);
    let mut cycle_context = member.context.clone().unwrap();
    cycle_context.requests[0].1["source"] = json!("opencode");
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(cursor),
        branch: "branch-a".into(),
        context: Some(cycle_context),
        change: PublishedEvidenceChange::ObservationUpsert(PublishedObservation {
            identity: "cycle".into(),
            native_order: NativeOrderKey("native-cycle".into()),
            first_eligible_publication: Some(PublicationCursor(cursor)),
            revision: 1,
            eligible: true,
            payload_complete: true,
            validity: crate::source::EvidenceValidity::Valid,
            payload: PublishedObservationPayload::ResponseCycle(PublishedResponseCycle {
                id: "cycle".into(),
                status: ResponseCycleStatus::Completed,
                messages: vec![member_message],
                attribution: Default::default(),
                native_reason: None,
                native_error_code: None,
                started_at_ms: None,
                ended_at_ms: None,
                dependencies: vec![PublishedSourceEvidence {
                    identity: "member".into(),
                    version: member_native_version,
                }],
            }),
        }),
    }
}

fn cycle_activation() -> EffectiveActivation {
    let mut activation = window_activation();
    activation.bootstrap_publication = PublicationCursor(100);
    activation.compiled = Arc::new(crate::compiler::compile("name: Complete cycle\non:\n  turn_end:\nactions:\n  - type: status\n    input: {result: failure}\n").unwrap());
    activation
}

#[test]
fn cycle_member_mutation_in_the_same_page_cancels_the_frozen_candidate() {
    for removal in [false, true] {
        let activation = cycle_activation();
        let member = native_member_publication(101, 1, 101);
        let cycle = cycle_publication(102, &member);
        let mutation = if removal {
            PublishedEvidenceEvent {
                source_evidence_version: None,
                cursor: PublicationCursor(103),
                branch: "branch-a".into(),
                context: None,
                change: PublishedEvidenceChange::Removal {
                    identity: "member".into(),
                    revision: 2,
                },
            }
        } else {
            native_member_publication(103, 2, 101)
        };
        let batch = PublishedEvidenceSource::new(JournalReader(vec![member, cycle, mutation]), 10)
            .unwrap()
            .read_batch(PublicationCursor(0))
            .unwrap();
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let outcome = engine.apply_source_page(&activation, &batch).unwrap();
        assert_eq!(outcome.candidates, 0);
        assert!(store.stored_candidates.lock().unwrap().is_empty());
        assert_eq!(
            store.progress.lock().unwrap().publication_cursor,
            PublicationCursor(103)
        );
    }
}

fn member_removal(cursor: u64, revision: u64) -> PublishedEvidenceEvent {
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(cursor),
        branch: "branch-a".into(),
        context: None,
        change: PublishedEvidenceChange::Removal {
            identity: "member".into(),
            revision,
        },
    }
}

#[test]
fn unseen_historical_cycle_member_mutations_cancel_same_page_candidates() {
    for removal in [false, true] {
        let activation = cycle_activation();
        let historical_member = native_member_publication(90, 1, 90);
        let cycle = cycle_publication(102, &historical_member);
        let mutation = if removal {
            member_removal(103, 2)
        } else {
            native_member_publication(103, 2, 90)
        };
        let source =
            PublishedEvidenceSource::new(JournalReader(vec![cycle, mutation]), 10).unwrap();
        let batch = source.read_batch(PublicationCursor(100)).unwrap();
        let store = Arc::new(FakeStore::default());
        store.progress.lock().unwrap().publication_cursor = PublicationCursor(100);
        assert!(
            store
                .progress
                .lock()
                .unwrap()
                .selection
                .seen_results
                .is_empty()
        );
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let outcome = engine.apply_source_page(&activation, &batch).unwrap();
        assert_eq!(outcome.candidates, 0, "removal: {removal}");
        assert!(store.stored_candidates.lock().unwrap().is_empty());
        assert!(engine.state.lock().unwrap().pending.is_empty());
        let progress = store.progress.lock().unwrap();
        assert_eq!(progress.publication_cursor, PublicationCursor(103));
        assert_eq!(
            progress.selection.seen_results[&(BranchId("branch-a".into()), "member".into())]
                .membership,
            if removal {
                crate::selection::ResultMembership::Removed
            } else {
                crate::selection::ResultMembership::Historical
            }
        );
    }
}

#[test]
fn unseen_cycle_member_upsert_with_the_exact_bound_version_keeps_the_candidate() {
    let activation = cycle_activation();
    let historical_member = native_member_publication(90, 1, 90);
    let cycle = cycle_publication(102, &historical_member);
    let mut unchanged = historical_member;
    unchanged.cursor = PublicationCursor(103);
    let batch = PublishedEvidenceSource::new(JournalReader(vec![cycle, unchanged]), 10)
        .unwrap()
        .read_batch(PublicationCursor(100))
        .unwrap();
    let store = Arc::new(FakeStore::default());
    store.progress.lock().unwrap().publication_cursor = PublicationCursor(100);
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.observations, 2);
    assert_eq!(outcome.candidates, 1);
    assert_eq!(store.stored_candidates.lock().unwrap().len(), 1);
    assert!(!engine.state.lock().unwrap().pending.is_empty());
}

#[test]
fn normalized_duplicate_native_token_changes_cancel_only_stale_cycle_members() {
    for restart_before_cycle in [false, true] {
        let mut expected_checkpoint = None;
        for token_change in ["equal", "changed", "missing"] {
            let activation = cycle_activation();
            let member = native_member_publication(101, 1, 101);
            let cycle = cycle_publication(102, &member);
            let mut duplicate = member.clone();
            duplicate.cursor = PublicationCursor(103);
            match token_change {
                "changed" => {
                    duplicate.source_evidence_version =
                        Some(EvidenceVersion("new-native-publication-token".into()))
                }
                "missing" => duplicate.source_evidence_version = None,
                _ => {}
            }
            let batch =
                PublishedEvidenceSource::new(JournalReader(vec![member, cycle, duplicate]), 10)
                    .unwrap()
                    .read_batch(PublicationCursor(0))
                    .unwrap();
            let (
                SourceEvent::Observation {
                    observation: first,
                    revision: first_revision,
                    ..
                },
                SourceEvent::Observation {
                    observation: duplicate,
                    revision: duplicate_revision,
                    ..
                },
            ) = (&batch.events[0], &batch.events[2])
            else {
                unreachable!()
            };
            assert_eq!(first, duplicate);
            assert_eq!(first_revision, duplicate_revision);
            let store = Arc::new(FakeStore::default());
            let mut engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
            let applied = if restart_before_cycle {
                assert_eq!(
                    engine
                        .apply_source_page(&activation, &page(vec![batch.events[0].clone()]))
                        .unwrap()
                        .observations,
                    1
                );
                drop(engine);
                let checkpoint = store.progress.lock().unwrap().encode().unwrap();
                *store.progress.lock().unwrap() = SourceProgress::decode(&checkpoint).unwrap();
                engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
                page(batch.events[1..].to_vec())
            } else {
                batch
            };
            let outcome = engine.apply_source_page(&activation, &applied).unwrap();
            assert_eq!(
                outcome.observations,
                if restart_before_cycle { 1 } else { 2 }
            );
            let expected_candidates = usize::from(token_change != "changed");
            assert_eq!(
                outcome.candidates, expected_candidates,
                "{token_change}, restart: {restart_before_cycle}"
            );
            assert_eq!(
                store.stored_candidates.lock().unwrap().len(),
                expected_candidates
            );
            assert_eq!(
                engine.state.lock().unwrap().pending.is_empty(),
                expected_candidates == 0
            );
            assert_eq!(store.committed.lock().unwrap().len(), 2);
            assert!(matches!(
                store.applied_events.lock().unwrap().last(),
                Some(SourceEvent::Ignored {
                    cursor: PublicationCursor(103)
                })
            ));
            let progress = store.progress.lock().unwrap();
            assert_eq!(progress.observation_position, ObservationPosition(2));
            assert_eq!(progress.publication_cursor, PublicationCursor(103));
            let checkpoint = progress.encode().unwrap();
            if let Some(expected) = &expected_checkpoint {
                assert_eq!(&checkpoint, expected);
            } else {
                expected_checkpoint = Some(checkpoint);
            }
        }
    }
}

#[test]
fn native_only_duplicates_do_not_cancel_normalized_message_dependencies() {
    let activation = window_activation();
    let first = native_member_publication(101, 1, 101);
    let second = publication(102, "other", 1, Some(102));
    let mut duplicate = first.clone();
    duplicate.cursor = PublicationCursor(103);
    duplicate.source_evidence_version =
        Some(EvidenceVersion("new-native-publication-token".into()));
    let batch = PublishedEvidenceSource::new(JournalReader(vec![first, second, duplicate]), 10)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.observations, 2);
    assert_eq!(outcome.candidates, 1);
    assert_eq!(store.stored_candidates.lock().unwrap().len(), 1);
}

#[test]
fn native_only_cycle_duplicates_do_not_cancel_their_own_normalized_binding() {
    let activation = cycle_activation();
    let member = native_member_publication(90, 1, 90);
    let mut cycle = cycle_publication(102, &member);
    cycle.source_evidence_version = Some(EvidenceVersion("cycle-native-token-a".into()));
    let mut duplicate = cycle.clone();
    duplicate.cursor = PublicationCursor(103);
    duplicate.source_evidence_version = Some(EvidenceVersion("cycle-native-token-b".into()));
    let batch = PublishedEvidenceSource::new(JournalReader(vec![cycle, duplicate]), 10)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.observations, 1);
    assert_eq!(outcome.candidates, 1);
    assert_eq!(store.stored_candidates.lock().unwrap().len(), 1);
}

#[test]
fn native_only_duplicate_invalidation_stays_in_its_source_branch() {
    let activation = cycle_activation();
    let member = native_member_publication(100, 1, 100);
    let mut other_branch = native_member_publication(101, 1, 100);
    other_branch.branch = "branch-b".into();
    other_branch.context.as_mut().unwrap().session_id = "session-b".into();
    let cycle = cycle_publication(102, &member);
    let mut duplicate = other_branch.clone();
    duplicate.cursor = PublicationCursor(103);
    duplicate.source_evidence_version = Some(EvidenceVersion("new-other-branch-token".into()));
    let batch = PublishedEvidenceSource::new(
        JournalReader(vec![member, other_branch, cycle, duplicate]),
        10,
    )
    .unwrap()
    .read_batch(PublicationCursor(0))
    .unwrap();
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.observations, 3);
    assert_eq!(outcome.candidates, 1);
    assert_eq!(store.stored_candidates.lock().unwrap().len(), 1);
}

#[test]
fn absent_native_version_does_not_prove_a_cycle_member_changed() {
    let activation = cycle_activation();
    let member = native_member_publication(90, 1, 90);
    let cycle = cycle_publication(102, &member);
    let mut correction = native_member_publication(103, 2, 90);
    correction.source_evidence_version = None;
    let batch = PublishedEvidenceSource::new(JournalReader(vec![cycle, correction]), 10)
        .unwrap()
        .read_batch(PublicationCursor(100))
        .unwrap();
    let store = Arc::new(FakeStore::default());
    store.progress.lock().unwrap().publication_cursor = PublicationCursor(100);
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.candidates, 1);
    assert_eq!(store.stored_candidates.lock().unwrap().len(), 1);
}

#[test]
fn a_cycle_own_correction_uses_its_normalized_version_without_a_native_token() {
    let activation = cycle_activation();
    let member = native_member_publication(90, 1, 90);
    let cycle = cycle_publication(102, &member);
    let mut correction = cycle.clone();
    correction.cursor = PublicationCursor(103);
    let PublishedEvidenceChange::ObservationUpsert(published) = &mut correction.change else {
        unreachable!()
    };
    published.revision = 2;
    let batch = PublishedEvidenceSource::new(JournalReader(vec![cycle, correction]), 10)
        .unwrap()
        .read_batch(PublicationCursor(100))
        .unwrap();
    let store = Arc::new(FakeStore::default());
    store.progress.lock().unwrap().publication_cursor = PublicationCursor(100);
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.candidates, 0);
    assert!(store.stored_candidates.lock().unwrap().is_empty());
    assert!(engine.state.lock().unwrap().pending.is_empty());
}

#[test]
fn unsafe_cycle_member_update_cancels_without_comparing_version_types() {
    let activation = cycle_activation();
    let member = native_member_publication(90, 1, 90);
    let cycle = cycle_publication(102, &member);
    let mut correction = native_member_publication(103, 2, 90);
    let PublishedEvidenceChange::Upsert(result) = &mut correction.change else {
        unreachable!()
    };
    result.validity = crate::source::EvidenceValidity::UnsafeOwnership;
    let batch = PublishedEvidenceSource::new(JournalReader(vec![cycle, correction]), 10)
        .unwrap()
        .read_batch(PublicationCursor(100))
        .unwrap();
    assert!(matches!(
        batch.events.last(),
        Some(SourceEvent::UnsafeUpdate { .. })
    ));
    let store = Arc::new(FakeStore::default());
    store.progress.lock().unwrap().publication_cursor = PublicationCursor(100);
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.candidates, 0);
    assert!(store.stored_candidates.lock().unwrap().is_empty());
    assert!(engine.state.lock().unwrap().pending.is_empty());
}

#[test]
fn message_corrections_compare_normalized_versions_even_when_native_tokens_match() {
    let activation = window_activation();
    let mut first = publication(101, "a", 1, Some(101));
    let second = publication(102, "b", 1, Some(102));
    let mut correction = publication(103, "a", 2, Some(101));
    let native_version = Some(EvidenceVersion("opaque-native-token".into()));
    first.source_evidence_version = native_version.clone();
    correction.source_evidence_version = native_version;
    let batch = PublishedEvidenceSource::new(JournalReader(vec![first, second, correction]), 10)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let store = Arc::new(FakeStore::default());
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine.apply_source_page(&activation, &batch).unwrap();
    assert_eq!(outcome.candidates, 0);
    assert!(store.stored_candidates.lock().unwrap().is_empty());
    assert!(engine.state.lock().unwrap().pending.is_empty());
}

#[test]
fn rejected_old_or_duplicate_cycle_member_mutations_keep_same_page_candidates() {
    for mutation_kind in ["duplicate", "old-upsert", "old-removal"] {
        let activation = cycle_activation();
        let current_member = native_member_publication(90, 3, 90);
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let initial = PublishedEvidenceSource::new(JournalReader(vec![current_member.clone()]), 10)
            .unwrap()
            .read_batch(PublicationCursor(0))
            .unwrap();
        assert_eq!(
            engine
                .apply_source_page(&activation, &initial)
                .unwrap()
                .candidates,
            0
        );
        let cycle = cycle_publication(102, &current_member);
        let mutation = match mutation_kind {
            "old-upsert" => native_member_publication(103, 2, 90),
            "old-removal" => member_removal(103, 2),
            _ => {
                let mut duplicate = current_member;
                duplicate.cursor = PublicationCursor(103);
                duplicate
            }
        };
        let batch = PublishedEvidenceSource::new(JournalReader(vec![cycle, mutation]), 10)
            .unwrap()
            .read_batch(PublicationCursor(90))
            .unwrap();
        let outcome = engine.apply_source_page(&activation, &batch).unwrap();
        assert_eq!(outcome.observations, 1, "{mutation_kind}");
        assert_eq!(outcome.candidates, 1, "{mutation_kind}");
        assert_eq!(store.stored_candidates.lock().unwrap().len(), 1);
        assert!(matches!(
            store.applied_events.lock().unwrap().last(),
            Some(SourceEvent::Ignored {
                cursor: PublicationCursor(103)
            })
        ));
    }
}

#[test]
fn normalized_lifecycles_keep_exact_candidates_across_pages_and_checkpoint_restart() {
    let mut activation = window_activation();
    Arc::make_mut(&mut activation.compiled).trigger.window = Some(RollingWindow {
        size: 3,
        minimum: 3,
    });
    let events = vec![
        publication(101, "a", 1, None),
        publication(102, "a", 2, None),
        publication(103, "b", 1, None),
        publication(104, "a", 3, None),
        publication(105, "b", 2, Some(105)),
        publication(106, "c", 1, None),
        publication(107, "a", 4, Some(107)),
        publication(108, "c", 2, None),
        publication(109, "b", 3, Some(105)),
        publication(110, "c", 3, Some(110)),
        publication(111, "d", 1, None),
        publication(112, "d", 2, None),
        publication(113, "d", 3, Some(113)),
        publication(114, "d", 3, Some(113)),
    ];
    let mut expected = None;
    for page_size in [1, 4, 1000] {
        let source =
            PublishedEvidenceSource::new(JournalReader(events.clone()), page_size).unwrap();
        let store = Arc::new(FakeStore::default());
        loop {
            let after = store.progress.lock().unwrap().publication_cursor;
            let batch = source.read_batch(after).unwrap();
            let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
            engine.apply_source_page(&activation, &batch).unwrap();
            drop(engine);
            let mut progress = store.progress.lock().unwrap();
            *progress = SourceProgress::decode(&progress.encode().unwrap()).unwrap();
            if !batch.has_more {
                break;
            }
        }
        let progress = store.progress.lock().unwrap();
        assert_eq!(progress.publication_cursor, PublicationCursor(114));
        assert_eq!(progress.observation_position, ObservationPosition(13));
        assert!(progress.selection.branches[&BranchId("branch-a".into())].coverage_complete);
        let candidates = store.stored_candidates.lock().unwrap().clone();
        assert_eq!(candidates.len(), 2, "page size {page_size}");
        for (candidate, identities) in candidates.iter().zip([["b", "a", "c"], ["a", "c", "d"]]) {
            assert_eq!(
                candidate
                    .messages
                    .iter()
                    .map(|message| message.id.as_str())
                    .collect::<Vec<_>>(),
                identities
            );
            assert_eq!(
                candidate.context.value["session"]["source_coverage"],
                "complete"
            );
        }
        assert_eq!(
            candidates[0].messages[0].value["tool"]["result"]["content"],
            "b output revision 3"
        );
        assert_eq!(candidates[0].trigger_position, ObservationPosition(10));
        if let Some(expected) = &expected {
            assert_eq!(&candidates, expected);
        } else {
            expected = Some(candidates);
        }
        assert!(store.accepted.lock().unwrap().is_empty());
        assert_eq!(*store.dispatches.lock().unwrap(), 0);
    }
}

#[test]
fn activation_boundary_uses_first_terminal_publication_after_restart() {
    let mut activation = window_activation();
    activation.bootstrap_publication = PublicationCursor(100);
    let source = PublishedEvidenceSource::new(
        JournalReader(vec![
            publication(101, "old", 30, Some(90)),
            publication(102, "pending", 1, None),
            publication(103, "pending", 2, None),
            publication(104, "pending", 3, Some(104)),
            publication(105, "old", 31, Some(90)),
            publication(106, "new", 1, Some(106)),
        ]),
        1,
    )
    .unwrap();
    let store = Arc::new(FakeStore::default());
    store.progress.lock().unwrap().publication_cursor = PublicationCursor(100);
    loop {
        let after = store.progress.lock().unwrap().publication_cursor;
        let batch = source.read_batch(after).unwrap();
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        engine.apply_source_page(&activation, &batch).unwrap();
        drop(engine);
        let mut progress = store.progress.lock().unwrap();
        *progress = SourceProgress::decode(&progress.encode().unwrap()).unwrap();
        if !batch.has_more {
            break;
        }
    }
    let candidates = store.stored_candidates.lock().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates[0]
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["pending", "new"]
    );
    assert_eq!(
        candidates[0].context.value["session"]["source_coverage"],
        "complete"
    );
}

#[test]
fn stale_removals_and_upserts_advance_delivery_without_replacing_committed_evidence() {
    let store = Arc::new(FakeStore::default());
    let activation = window_activation();
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine
        .apply_source_page(
            &activation,
            &page(vec![
                source_event_revision(1, "a", 3),
                SourceEvent::Removal {
                    cursor: PublicationCursor(2),
                    branch: BranchId("branch-a".into()),
                    identity: "a".into(),
                    revision: 2,
                },
                source_event_revision(3, "a", 2),
                source_event_revision(4, "a", 3),
                source_event(5, "b"),
            ]),
        )
        .unwrap();
    assert_eq!(outcome.observations, 2);
    assert_eq!(outcome.candidates, 1);
    let committed = store.committed.lock().unwrap();
    assert_eq!(
        committed
            .iter()
            .map(|item| item.revision)
            .collect::<Vec<_>>(),
        [3, 1]
    );
    let events = store.applied_events.lock().unwrap();
    assert!(
        events[1..4]
            .iter()
            .all(|event| matches!(event, SourceEvent::Ignored { .. }))
    );
    assert_eq!(
        store.progress.lock().unwrap().publication_cursor,
        PublicationCursor(5)
    );
}

#[test]
fn removal_revision_survives_checkpoint_restart_and_blocks_equal_revision_restore() {
    let store = Arc::new(FakeStore::default());
    let activation = window_activation();
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    engine
        .apply_source_page(
            &activation,
            &page(vec![
                source_event_revision(1, "a", 3),
                SourceEvent::Removal {
                    cursor: PublicationCursor(2),
                    branch: BranchId("branch-a".into()),
                    identity: "a".into(),
                    revision: 3,
                },
            ]),
        )
        .unwrap();
    drop(engine);
    let saved = store.progress.lock().unwrap().encode().unwrap();
    *store.progress.lock().unwrap() = SourceProgress::decode(&saved).unwrap();
    let restarted = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = restarted
        .apply_source_page(
            &activation,
            &page(vec![
                source_event_revision(3, "a", 3),
                source_event_revision(4, "a", 2),
                source_event_revision(5, "a", 4),
                source_event(6, "b"),
                source_event(7, "c"),
            ]),
        )
        .unwrap();
    assert_eq!(outcome.observations, 3);
    assert_eq!(outcome.candidates, 1);
    let candidates = store.stored_candidates.lock().unwrap();
    assert_eq!(
        candidates[0]
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
}

#[test]
fn newly_matching_correction_invalidates_membership_even_without_a_message_dependency() {
    let mut activation = window_activation();
    Arc::make_mut(&mut activation.compiled).trigger.tools = Some(vec![ToolCategory::Shell]);
    for split_page in [false, true] {
        let store = Arc::new(FakeStore::default());
        let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
        let mut excluded = source_event(1, "excluded");
        let SourceEvent::Observation { observation, .. } = &mut excluded else {
            unreachable!()
        };
        let ObservationKind::ToolResult { category, .. } = &mut observation.kind else {
            unreachable!()
        };
        *category = "read".into();
        let initial = vec![excluded, source_event(2, "a"), source_event(3, "b")];
        let correction = source_event_revision(4, "excluded", 2);
        let outcome = if split_page {
            assert_eq!(
                engine
                    .apply_source_page(&activation, &page(initial))
                    .unwrap()
                    .candidates,
                1
            );
            engine
                .apply_source_page(&activation, &page(vec![correction]))
                .unwrap()
        } else {
            engine
                .apply_source_page(
                    &activation,
                    &page(initial.into_iter().chain([correction]).collect()),
                )
                .unwrap()
        };
        assert_eq!(outcome.candidates, 0);
        let committed = store.committed.lock().unwrap();
        assert!(committed.last().unwrap().window_invalidated);
        assert!(committed[..3].iter().all(|item| !item.window_invalidated));
        if !split_page {
            assert!(store.stored_candidates.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn filter_diagnostics_commit_with_progress_and_corrections_do_not_retrigger_after_restart() {
    let store = Arc::new(FakeStore::default());
    let mut activation = window_activation();
    Arc::make_mut(&mut activation.compiled).trigger.fast_mode = Some(false);
    let engine = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = engine
        .apply_source_page(&activation, &page(vec![source_event(1, "a")]))
        .unwrap();
    assert_eq!(outcome.candidates, 0);
    assert_eq!(outcome.filter_outcomes.len(), 1);
    assert_eq!(
        outcome.filter_outcomes[0].1.field,
        crate::filters::FilterField::FastMode
    );
    drop(engine);
    let saved = store.progress.lock().unwrap().encode().unwrap();
    *store.progress.lock().unwrap() = SourceProgress::decode(&saved).unwrap();
    assert!(
        store.progress.lock().unwrap().selection.branches[&BranchId("branch-a".into())]
            .last_filter_diagnostic
            .is_some()
    );
    let standard_mode = |mut event| {
        let SourceEvent::Observation { observation, .. } = &mut event else {
            unreachable!()
        };
        let ObservationKind::ToolResult { message, .. } = &mut observation.kind else {
            unreachable!()
        };
        message.value["fast_mode"] = json!(false);
        event
    };
    let restarted = CheckEngine::new(store.clone(), Arc::new(FakeEvaluator));
    let outcome = restarted
        .apply_source_page(
            &activation,
            &page(vec![
                standard_mode(source_event_revision(2, "a", 2)),
                standard_mode(source_event(3, "b")),
                standard_mode(source_event(4, "c")),
            ]),
        )
        .unwrap();
    assert_eq!(outcome.candidates, 1);
    assert!(outcome.filter_outcomes.is_empty());
    assert_eq!(
        store.stored_candidates.lock().unwrap()[0]
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    let correction = restarted
        .apply_source_page(&activation, &page(vec![source_event_revision(5, "b", 2)]))
        .unwrap();
    assert_eq!(correction.candidates, 0);
    assert_eq!(correction.filter_outcomes.len(), 1);
    assert_eq!(
        store.progress.lock().unwrap().publication_cursor,
        PublicationCursor(5)
    );
}
