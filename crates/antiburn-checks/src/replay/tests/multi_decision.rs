use super::*;

fn active() -> EffectiveActivation {
    activation_for(
        "name: Two decisions\non:\n  tool_result:\ndecisions:\n  review:\n    input: {requests: '${{ session.requests }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {stuck: Concern., clear: Clear., uncertain: Unclear.}\n  second:\n    input: {requests: '${{ session.requests }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this complete?\n        criteria: {stuck: Concern., clear: Clear., uncertain: Unclear.}\nactions:\n  - type: status\n    input: {result: success}\n",
    )
}

fn run(
    store: Arc<MemoryStore>,
    outcomes: Vec<Result<&'static str, AttemptFailure>>,
    attempts: usize,
) -> ReplayReport {
    block_on(replay(
        ReplayRunId("multi".into()),
        active(),
        store,
        Arc::new(SequenceEvaluator(Mutex::new(outcomes.into()))),
        &mut Events(vec![event(1)].into_iter()),
        ReplayLimits {
            events: 1,
            evaluations: 1,
            attempts,
        },
        || false,
    ))
    .unwrap()
}

#[test]
fn replay_checks_the_allowance_before_each_block_and_each_retry() {
    for (allowance, outcomes, sends, end, accepted) in [
        (1, vec![], vec![], ReplayEnd::LimitReached, 0),
        (
            2,
            vec![Ok("clear"), Ok("clear")],
            vec![1, 1],
            ReplayEnd::Complete,
            1,
        ),
        (
            2,
            vec![Err(AttemptFailure::SafeTransient), Ok("clear")],
            vec![1, 2],
            ReplayEnd::LimitReached,
            0,
        ),
        (
            3,
            vec![Ok("clear"), Err(AttemptFailure::SafeTransient), Ok("clear")],
            vec![1, 1, 2],
            ReplayEnd::Complete,
            1,
        ),
    ] {
        let store = Arc::new(MemoryStore::default());
        let report = run(store.clone(), outcomes, allowance);
        assert_eq!(report.end, end);
        assert_eq!(report.attempts, sends.len());
        assert_eq!(*store.dispatches.lock().unwrap(), sends);
        assert_eq!(store.accepted.lock().unwrap().len(), accepted);
    }
}

#[test]
fn replay_keeps_known_counts_and_stops_on_uncertain_intent_commit() {
    let store = Arc::new(MemoryStore::default());
    *store.fail_intent_on_call.lock().unwrap() = Some(3);
    let report = run(
        store.clone(),
        vec![Ok("clear"), Err(AttemptFailure::SafeTransient)],
        3,
    );
    assert_eq!(report.attempts, 2);
    assert!(
        matches!(report.end, ReplayEnd::ExecutionError(ref detail) if detail.contains("uncertain"))
    );
    assert_eq!(*store.dispatches.lock().unwrap(), [1, 1, 2]);
    assert!(store.accepted.lock().unwrap().is_empty());
}

#[test]
fn replay_keeps_all_committed_counts_when_acceptance_storage_fails() {
    let store = Arc::new(MemoryStore::default());
    *store.fail_accept.lock().unwrap() = true;
    let report = run(store.clone(), vec![Ok("clear"), Ok("clear")], 2);
    assert_eq!(report.attempts, 2);
    assert!(
        matches!(report.end, ReplayEnd::ExecutionError(ref detail) if detail.contains("acceptance"))
    );
    assert!(store.accepted.lock().unwrap().is_empty());
    assert_eq!(store.answers.lock().unwrap().len(), 1);
}

#[test]
fn unknown_second_delivery_stops_replay_without_partial_actions() {
    let store = Arc::new(MemoryStore::default());
    let report = run(
        store.clone(),
        vec![Ok("clear"), Err(AttemptFailure::Unknown)],
        4,
    );
    assert_eq!(report.attempts, 2);
    assert!(matches!(report.end, ReplayEnd::ExecutionError(_)));
    assert_eq!(*store.dispatches.lock().unwrap(), [1, 1]);
    assert!(store.accepted.lock().unwrap().is_empty());
    assert!(store.answers.lock().unwrap().is_empty());
}
