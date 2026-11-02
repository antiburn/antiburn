use super::*;
use crate::jev::config::SystemOneConnection;
use crate::jev::decision_evaluator::TypeSafeDecisionEvaluator;
use antiburn_checks::decision::DecisionInput;
use antiburn_checks::engine::DecisionEvaluator;
use antiburn_local::analysis::jev::NATIVE_MODEL_ALIAS;

#[test]
fn evidence_keeps_the_request_alias_and_each_observed_attempt_on_the_writing_assessment() {
    let (store, definition) = populated();
    let connection = SystemOneConnection::jev_default();
    let evaluator = TypeSafeDecisionEvaluator::from_selected_snapshot(
        connection.clone(),
        Some("synthetic-unused-key".into()),
        connection.capabilities().unwrap(),
    )
    .unwrap();
    let activation = EffectiveActivation {
        id: ActivationId {
            scope: scope("/workspace/demo"),
            definition_id: definition.check_id.clone(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Project,
        source_paths: vec![],
        project_root: Some("/workspace/demo".into()),
        compiled: Arc::new(compile(REPORT_CHECK).unwrap()),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: Default::default(),
    };
    let prepared = evaluator
        .prepare(
            &activation,
            vec![DecisionInput {
                decision: "review".into(),
                input: json!({"activity":"error"}),
            }],
            Default::default(),
        )
        .unwrap();
    let encoded =
        serde_json::to_string(&antiburn_checks::fit::PreparationRecord::Ready(prepared)).unwrap();
    {
        let connection = store.lock();
        connection
            .execute(
                "UPDATE burn_check_candidate SET preparation_json = ?1",
                [encoded],
            )
            .unwrap();
        for (assessment, attempt_id, number, model) in [
            ("assessment-1", "native-attempt-a", 1, "jev-1.13.0"),
            ("assessment-1", "native-attempt-b", 2, "jev-1.14.0"),
            ("assessment-2", "later-attempt", 1, "unrelated-future-model"),
        ] {
            connection.execute(
                "INSERT INTO burn_check_evaluation_attempt
                    (candidate_id, attempt_id, decision_name, attempt_number, state, outcome, updated_at_epoch)
                 SELECT candidate_id, ?2, 'review', ?3, 'settled', 'answered', 110
                   FROM burn_check_assessment_history WHERE assessment_id = ?1",
                params![assessment, attempt_id, number],
            ).unwrap();
            connection.execute(
                "INSERT INTO burn_check_decision_usage (attempt_id, usage_known, served_model) VALUES (?1, 1, ?2)",
                params![attempt_id, model],
            ).unwrap();
        }
    }
    let current = rows(&store, std::slice::from_ref(&definition));
    let evidence =
        read_evidence(&store, &[definition], &request(), &current[0].targets[0].id).unwrap();
    let item = evidence
        .items
        .iter()
        .find(|item| item.reference == "decision-connection")
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&item.excerpt).unwrap();
    assert_eq!(value["requestedModel"], NATIVE_MODEL_ALIAS);
    assert_eq!(value["modelRevision"], serde_json::Value::Null);
    assert_eq!(
        value["usageObservations"],
        json!({
            "attempts":[
                {"decision":"review", "attempt":1, "usageKnown":true, "servedModel":"jev-1.13.0"},
                {"decision":"review", "attempt":2, "usageKnown":true, "servedModel":"jev-1.14.0"}
            ],
            "truncated":false
        })
    );
    let serialized = serde_json::to_string(&evidence).unwrap();
    for excluded in [
        "synthetic-unused-key",
        "native-attempt-a",
        "native-attempt-b",
        "unrelated-future-model",
    ] {
        assert!(!serialized.contains(excluded));
    }
    assert!(item.explanation.contains("each decision and attempt"));
}
