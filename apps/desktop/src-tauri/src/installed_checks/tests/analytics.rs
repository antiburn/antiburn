use super::*;
use antiburn_checks::fit::{
    FitOmission, FitReport, FitStrategy, LocalPreparation, PreparationRecord,
};
use antiburn_checks::prepared_assessment::{PreparedAssessment, PreparedBlock};

#[test]
fn local_preparation_does_not_log_or_emit_a_model_preparation_event() {
    let preparation = PreparationRecord::Local(LocalPreparation::new(1001));
    let captured = LogCapture(Arc::default(), "installed_check_input_prepared");
    tracing::subscriber::with_default(captured.clone(), || {
        record_preparation(&preparation, |_| {
            panic!("local work has no model preparation outcome")
        });
    });
    assert!(captured.0.lock().unwrap().is_empty());
}

#[test]
fn preparation_logs_only_metadata_and_emits_one_categorical_outcome() {
    let preparation = PreparationRecord::Ready(PreparedAssessment {
        version: 1,
        profile: "private profile".into(),
        evaluation_budget: Default::default(),
        fitting_work_bytes: 0,
        decisions: vec![PreparedBlock {
            decision: "review".into(),
            original_input: json!({"private input name": "private transcript"}),
            input: json!({"private input name": "private transcript"}),
            request: json!({"private request": "private YAML value"}),
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
        }],
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
