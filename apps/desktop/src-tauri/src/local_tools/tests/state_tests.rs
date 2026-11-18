use super::*;

fn ready() -> Snapshot {
    Snapshot {
        status: Status {
            revision: 0,
            phase: Phase::Ready,
            cli_installed: true,
            cli_tested: true,
            skill_installed: true,
            reason: None,
        },
        tested_sha256: Some("same-digest".into()),
    }
}

fn ready_state() -> State {
    let mut state = State::new(Status::initial(None));
    let (_, worker) = state.begin_setup();
    state.finish_setup(worker.unwrap(), ready());
    state
}

#[test]
fn old_ready_inspection_cannot_replace_a_new_failed_setup_with_unchanged_bytes() {
    let mut state = ready_state();
    let old_revision = state.snapshot.status.revision;
    let (_, worker) = state.begin_setup();
    let mut failed = ready();
    failed.status = Status::failed(Reason::SetupUnavailable, true, true);
    let (latest, _) = state.finish_setup(worker.unwrap(), failed);
    assert_eq!(state.snapshot.tested_sha256.as_deref(), Some("same-digest"));
    let (response, changed) = state.apply_inspection(old_revision, Ok(()));
    assert!(!changed);
    assert_eq!(response.revision, latest.revision);
    assert_eq!(response.phase, Phase::Failed);
}

#[test]
fn old_ready_inspection_returns_current_pending_state_during_repair() {
    let mut state = ready_state();
    let old_revision = state.snapshot.status.revision;
    let (pending, _) = state.begin_setup();
    let (response, changed) = state.apply_inspection(old_revision, Ok(()));
    assert!(!changed);
    assert_eq!(response.revision, pending.revision);
    assert_eq!(response.phase, Phase::Pending);
}

#[test]
fn queued_work_never_publishes_its_superseded_ready_result() {
    let mut state = ready_state();
    let (first, worker) = state.begin_setup();
    let worker = worker.unwrap();
    let (queued, second_worker) = state.begin_setup();
    assert!(second_worker.is_none());
    assert!(queued.revision > first.revision);
    let (rerun, again) = state.finish_setup(worker, ready());
    assert!(again);
    assert_eq!(rerun.phase, Phase::Pending);
    assert!(rerun.revision > queued.revision);
    let (finished, again) = state.finish_setup(worker, ready());
    assert!(!again);
    assert_eq!(finished.phase, Phase::Ready);
    assert!(finished.revision > rerun.revision);
}

#[test]
fn old_worker_cleanup_cannot_cancel_a_new_worker() {
    let mut state = ready_state();
    let (_, old_worker) = state.begin_setup();
    state.finish_setup(old_worker.unwrap(), ready());
    let (pending, new_worker) = state.begin_setup();
    assert!(state.abort_setup(old_worker.unwrap()).is_none());
    assert_eq!(state.snapshot.status.revision, pending.revision);
    assert!(state.abort_setup(new_worker.unwrap()).is_some());
    assert_eq!(state.snapshot.status.phase, Phase::Failed);
}

#[test]
fn inspection_failure_advances_revision_and_rejects_an_older_success() {
    let mut state = ready_state();
    let old_revision = state.snapshot.status.revision;
    let (failure, changed) = state.apply_inspection(old_revision, Err(Reason::OwnedFileChanged));
    assert!(changed);
    assert!(failure.revision > old_revision);
    let (response, changed) = state.apply_inspection(old_revision, Ok(()));
    assert!(!changed);
    assert_eq!(response.phase, Phase::Failed);
    assert_eq!(response.revision, failure.revision);
}
