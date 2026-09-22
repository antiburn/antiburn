use super::*;

#[test]
fn a_cancel_between_phases_stops_the_reduction_and_keeps_evidence_intact() {
    let data_dir = TempDir::new().unwrap();
    let store = Store::open(data_dir.path()).unwrap();
    publish_ready(&store, "ready", 120);
    let key = SessionKey::new("native", "claude-code", "ready");
    let before = store.evidence(&key).unwrap().unwrap();

    let cancel = AtomicBool::new(false);
    let error = reduce_on_snapshot(
        data_dir.path(),
        request(),
        &mut || cancel.store(true, Ordering::SeqCst),
        &cancel,
    )
    .unwrap_err();
    assert!(is_cancelled(&error));

    // The durable evidence state is untouched: the store still
    // opens and the row reads back unchanged.
    let after = store.evidence(&key).unwrap().unwrap();
    assert_eq!(after.status, before.status);
    assert_eq!(after.evidence_json, before.evidence_json);
    assert_eq!(after.claim_fence, before.claim_fence);

    // A fresh reduction succeeds after the cancelled one.
    let report = reduce_on_snapshot(
        data_dir.path(),
        request(),
        &mut || {},
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(report.assessed_sessions, 1);
}

#[test]
fn an_already_cancelled_request_stops_before_it_opens_a_snapshot() {
    let data_dir = TempDir::new().unwrap();
    let store = Store::open(data_dir.path()).unwrap();
    publish_ready(&store, "ready", 120);

    // The flag is set before the call, so the first probe stops
    // the reduction.
    let cancel = AtomicBool::new(true);
    let error = reduce_on_snapshot(data_dir.path(), request(), &mut || {}, &cancel).unwrap_err();
    assert!(is_cancelled(&error));
}

#[test]
fn cancellation_during_turn_iteration_stops_without_publishing_a_report() {
    let data_dir = TempDir::new().unwrap();
    let store = Store::open(data_dir.path()).unwrap();
    publish_evidence_with_turns(&store, "large", 120, PublishedEvidence::Ready, 100);
    let key = SessionKey::new("native", "claude-code", "large");
    let before = store.evidence(&key).unwrap().unwrap();
    let cancel = AtomicBool::new(false);
    let mut turns_scanned = 0;

    let error = reduce_with_state_on_snapshot(
        data_dir.path(),
        request(),
        &mut || {},
        &cancel,
        &mut || {
            turns_scanned += 1;
            if turns_scanned == 10 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        None,
    )
    .unwrap_err();

    assert!(is_cancelled(&error));
    assert_eq!(turns_scanned, 10);
    let after = store.evidence(&key).unwrap().unwrap();
    assert_eq!(after.evidence_json, before.evidence_json);
    assert_eq!(after.published_fence, before.published_fence);
}

#[test]
fn cancellation_before_turn_finalization_stops_without_publishing_a_report() {
    let data_dir = TempDir::new().unwrap();
    let store = Store::open(data_dir.path()).unwrap();
    publish_evidence_with_turns(&store, "large", 120, PublishedEvidence::Ready, 100);
    let cancel = AtomicBool::new(false);
    let mut probes = 0;

    let error = reduce_with_state_on_snapshot(
        data_dir.path(),
        request(),
        &mut || {},
        &cancel,
        &mut || {
            probes += 1;
            if probes == 101 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        None,
    )
    .unwrap_err();

    assert!(is_cancelled(&error));
    assert_eq!(probes, 101);
}

#[test]
fn cancellation_during_finalization_stops_without_publishing_a_report() {
    let data_dir = TempDir::new().unwrap();
    let store = Store::open(data_dir.path()).unwrap();
    publish_evidence_with_turns(&store, "large", 120, PublishedEvidence::Ready, 100);
    let cancel = AtomicBool::new(false);
    let mut probes = 0;

    let error = reduce_with_state_on_snapshot(
        data_dir.path(),
        request(),
        &mut || {},
        &cancel,
        &mut || {
            probes += 1;
            if probes == 102 {
                cancel.store(true, Ordering::SeqCst);
            }
        },
        None,
    )
    .unwrap_err();

    assert!(is_cancelled(&error));
    assert_eq!(probes, 102);
}
