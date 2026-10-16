use super::*;
use antiburn_checks::fit::{
    FitOmission, FitReport, FitStrategy, PreparationRecord, PreparedDecision,
};

#[test]
fn preparation_logs_only_metadata_and_emits_one_categorical_outcome() {
    let preparation = PreparationRecord::Ready(PreparedDecision {
        profile: "private profile".into(),
        input: json!({"private input name": "private transcript"}),
        request: Some(json!({"private request": "private YAML value"})),
        report: FitReport {
            omissions: vec![
                FitOmission {
                    path: "private.array[*]".into(),
                    strategy: FitStrategy::Recent,
                    original_count: 7,
                    retained_count: 3,
                },
                FitOmission {
                    path: "private.text".into(),
                    strategy: FitStrategy::Truncate,
                    original_count: 1300,
                    retained_count: 280,
                },
            ],
        },
    });
    let captured = LogCapture(Arc::default(), "installed_check_input_prepared");
    let mut outcomes = Vec::new();
    tracing::subscriber::with_default(captured.clone(), || {
        record_preparation(&preparation, |outcome| outcomes.push(outcome));
    });
    assert_eq!(
        outcomes,
        [crate::analytics::event::BurnCheckPreparationOutcome::Shortened]
    );
    let logs = captured.0.lock().unwrap();
    assert_eq!(logs.len(), 1);
    for expected in [
        "outcome=\"shortened\"",
        "shortened_targets=2",
        "removed_items=4",
        "removed_text_bytes=1020",
    ] {
        assert!(logs[0].contains(expected), "{expected}: {}", logs[0]);
    }
    assert!(!logs[0].contains("private"));
    assert!(!logs[0].contains("1300"));
    assert!(!logs[0].contains("280"));
}
