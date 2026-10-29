use super::*;
use crate::fit::{
    FitOmission, FitStrategy, FitValidationError, FittingWorkBudget, PreparationBudgetKind,
    fit_input_with_work_budget,
};
use crate::preparation::{bounded_preparation_record, decode_preparation_record};

#[test]
fn fitting_work_is_shared_across_blocks_and_saved_validation_is_separate() {
    let input = json!({"text": "selected"});
    let bytes = serde_json::to_vec(&input).unwrap().len() as u64;
    let mut work = FittingWorkBudget::default();
    work.charge(FittingWorkBudget::MAXIMUM - 3 * bytes).unwrap();
    let fit = |work: &mut FittingWorkBudget| {
        fit_input_with_work_budget(input.clone(), &[], work, |input, _, work| {
            work.measure(input)
                .map(|_| ())
                .map_err(|error| FitValidationError::ResourceLimit(error.context.budget.unwrap()))
        })
    };
    fit(&mut work).unwrap();
    assert_eq!(work.consumed(), FittingWorkBudget::MAXIMUM - bytes);
    let error = fit(&mut work).unwrap_err();
    assert_eq!(error.code, PreparationFailureCode::PreparationResourceLimit);
    assert_eq!(
        error.context.budget.unwrap().kind,
        PreparationBudgetKind::WorkBytes
    );
    assert!(work.consumed() > FittingWorkBudget::MAXIMUM);
    assert!(work.charge(0).is_err());
    let mut validation = FittingWorkBudget::default();
    validation.measure(&input).unwrap();
    validation.measure(&input).unwrap();
    assert_eq!(validation.consumed(), 2 * bytes);
}

#[test]
fn strict_record_decode_rejects_legacy_missing_budget_duplicates_and_versions() {
    let active = activation();
    let prepared = test_prepare(
        &active,
        vec![DecisionInput {
            decision: "review".into(),
            input: json!({"text": "selected"}),
        }],
        EvaluationBudget::default(),
    )
    .unwrap();
    let record = PreparationRecord::Ready(prepared);
    let bytes = serde_json::to_vec(&record).unwrap();
    assert_eq!(decode_preparation_record(&bytes).unwrap(), record);
    let value = serde_json::to_value(&record).unwrap();
    for key in ["version", "evaluation_budget", "fitting_work_bytes"] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(key);
        assert_eq!(
            decode_preparation_record(&serde_json::to_vec(&invalid).unwrap())
                .unwrap_err()
                .code,
            PreparationFailureCode::IncompatibleProfile
        );
    }
    let mut invalid = value;
    invalid["version"] = json!(2);
    assert!(decode_preparation_record(&serde_json::to_vec(&invalid).unwrap()).is_err());
    for bytes in [br#"{"status":"ready","profile":"legacy","input":{},"request":null,"report":{"omissions":[]}}"#.as_slice(), br#"{"status":"local","version":1,"version":1,"started_at_unix_ms":1,"evaluation_budget":{}}"#.as_slice()] {
        assert!(decode_preparation_record(bytes).is_err());
    }
    let mut raw_oversize =
        vec![b' '; crate::prepared_assessment::PREPARATION_LIMITS.max_bytes as usize];
    raw_oversize.extend_from_slice(&bytes);
    assert!(decode_preparation_record(&raw_oversize).is_err());
}

#[test]
fn oversized_no_fit_report_becomes_a_small_typed_resource_rejection() {
    let failure = PreparationFailure {
        code: PreparationFailureCode::NoFit,
        detail: "All permitted reductions are exhausted.".into(),
        report: Some(FitReport {
            omissions: (0..crate::fit::MAX_FIT_TARGETS)
                .map(|index| FitOmission {
                    path: format!(
                        "{}.items[{index}]",
                        "a".repeat(crate::fit::MAX_FIT_PATH_BYTES - 16)
                    ),
                    strategy: FitStrategy::Truncate,
                    original_count: 4096,
                    retained_count: 256,
                })
                .collect(),
        }),
        context: Default::default(),
    };
    let record = PreparationRecord::Rejected(failure);
    assert!(
        crate::bounded_json::measure_json(&record, crate::prepared_assessment::PREPARATION_LIMITS)
            .is_err()
    );
    let bounded = bounded_preparation_record(record);
    let PreparationRecord::Rejected(failure) = &bounded else {
        panic!("oversized failure must remain rejected");
    };
    assert_eq!(
        failure.code,
        PreparationFailureCode::PreparationResourceLimit
    );
    assert!(failure.report.is_none());
    let bytes = serde_json::to_vec(&bounded).unwrap();
    assert!(bytes.len() < 1024);
    assert_eq!(decode_preparation_record(&bytes).unwrap(), bounded);
}

#[test]
fn local_and_small_rejected_records_keep_their_exact_payload() {
    for record in [
        PreparationRecord::Local(crate::fit::LocalPreparation::new(123)),
        PreparationRecord::Rejected(incompatible_preparation(
            "The saved provider profile does not match.",
        )),
    ] {
        assert_eq!(bounded_preparation_record(record.clone()), record);
        assert_eq!(
            decode_preparation_record(&serde_json::to_vec(&record).unwrap()).unwrap(),
            record
        );
    }
}
