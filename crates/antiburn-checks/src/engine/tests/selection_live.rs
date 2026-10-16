use super::*;
use crate::compiler::{RollingWindow, ToolCategory};
use crate::source::{
    FileEventSource, PublicationRead, PublishedContext, PublishedEvidenceBatch,
    PublishedEvidenceChange, PublishedEvidenceEvent, PublishedEvidenceReader,
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
        cursor: PublicationCursor(cursor),
        branch: "branch-a".into(),
        context: Some(PublishedContext {
            requests: vec![("request".into(), json!({"text": "Inspect the failure."}))],
            session_id: "session-a".into(),
            agent_name: "opencode".into(),
            project_root: None,
            version: format!("context-{}", terminal.unwrap_or(cursor)),
            main_session: true,
            complete_through_event: terminal.is_some(),
        }),
        change: PublishedEvidenceChange::Upsert(PublishedToolResult {
            identity: identity.into(),
            native_order: NativeOrderKey(format!("native-{}", terminal.unwrap_or(cursor))),
            first_terminal_publication: terminal.map(PublicationCursor),
            revision,
            tool_name: Some("bash".into()),
            category: Some("shell".into()),
            arguments: Some(json!({"command": "cargo check"})),
            content: terminal.map(|_| format!("{identity} output revision {revision}")),
            status: PublishedResultStatus::Error,
            finality: terminal.is_some(),
            complete: terminal.is_some(),
            selection_safe: true,
        }),
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
        let source = FileEventSource::new(JournalReader(events.clone()), page_size).unwrap();
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
    let source = FileEventSource::new(
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
