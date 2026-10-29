use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use antiburn_checks::source::PublishedEvidenceSource;

#[test]
fn empty_project_progress_is_durable_without_changing_selection_or_pending_work() {
    let (store, adapter, active) = fixture();
    let candidate = apply(&adapter, &active, &event(&store, 1, "one", "branch", 1));
    let before = adapter.load_source_progress(&active).unwrap();
    event(&store, 2, "other", "other-branch", 1);
    store
        .lock()
        .execute(
            "UPDATE session SET cwd = '/other' WHERE session_id = 'other-branch'",
            [],
        )
        .unwrap();

    let source = PublishedEvidenceSource::new(
        StorePublishedEvidenceReader::for_live_project(&store, "native", "/repo")
            .with_activation(&active.id),
        1,
    )
    .unwrap();
    let page = source.read_batch(before.publication_cursor).unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.next, PublicationCursor(2));
    assert!(!page.has_more);
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    assert_eq!(
        engine.apply_source_page(&active, &page).unwrap().candidates,
        0
    );
    let mut expected = before;
    expected.publication_cursor = page.next;
    assert_eq!(adapter.load_source_progress(&active).unwrap(), expected);
    assert_eq!(adapter.load_recovery(None, 10).unwrap().len(), 1);
    assert!(matches!(
        adapter.claim_due(&candidate, 100_000, false).unwrap(),
        CandidateClaim::Claimed(_)
    ));

    let restarted =
        StoreCheckStore::new(store.clone(), "owner-2".into(), active.id.scope.clone()).unwrap();
    restarted.register_activation(&active).unwrap();
    assert_eq!(restarted.load_source_progress(&active).unwrap(), expected);
    let repeated = source.read_batch(page.next).unwrap();
    assert!(repeated.events.is_empty());
    assert_eq!(repeated.next, page.next);
    assert!(!repeated.has_more);

    let next = event(&store, 3, "two", "branch", 1);
    assert_eq!(
        engine.apply_source_page(&active, &next).unwrap().candidates,
        1
    );
    let progress = adapter.load_source_progress(&active).unwrap();
    assert_eq!(progress.publication_cursor, PublicationCursor(3));
    assert_eq!(progress.observation_position, ObservationPosition(52));
    let rewind = SourceBatch {
        events: vec![],
        next: PublicationCursor(2),
        has_more: false,
    };
    assert!(engine.apply_source_page(&active, &rewind).is_err());
    assert_eq!(adapter.load_source_progress(&active).unwrap(), progress);
}

#[test]
fn empty_project_progress_cannot_skip_matching_events_or_uncommitted_sequences() {
    let (store, adapter, active) = fixture();
    let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
    let before = adapter.load_source_progress(&active).unwrap();
    let matching = event(&store, 1, "one", "branch", 1);
    for next in [1, 2] {
        let page = SourceBatch {
            events: vec![],
            next: PublicationCursor(next),
            has_more: false,
        };
        assert!(engine.apply_source_page(&active, &page).is_err());
        assert_eq!(adapter.load_source_progress(&active).unwrap(), before);
    }
    let mut invalid = matching.clone();
    invalid.next = PublicationCursor(2);
    assert!(engine.apply_source_page(&active, &invalid).is_err());
    assert_eq!(adapter.load_source_progress(&active).unwrap(), before);
    engine.apply_source_page(&active, &matching).unwrap();
    let applied = adapter.load_source_progress(&active).unwrap();
    let ahead = SourceBatch {
        events: vec![],
        next: PublicationCursor(2),
        has_more: false,
    };
    assert!(engine.apply_source_page(&active, &ahead).is_err());
    assert_eq!(adapter.load_source_progress(&active).unwrap(), applied);
}
