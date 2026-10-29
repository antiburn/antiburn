use antiburn_checks::{
    actions::plan_status_actions,
    compile,
    compiler::StatusResult,
    results::{ChoiceResult, DecisionResult, EvaluatedAssessment, NamedAnswer, QuestionResult},
};
use antiburn_checks_store::{CheckStatus, Eligibility, read_session_checks, status_message};
use antiburn_local::analysis::PARSER_REVISION;
use rusqlite::{Connection, params};
use serde_json::json;

const SCOPE: &str = r#"{"environment":"native","session_cwd":"/repo","run":"Live"}"#;
const BRANCH: &str = r#"["native","claude-code","s1"]"#;
const CHECK: &str = "name: Saved typed status
on:
  tool_result:
decisions:
  first:
    input: {note: first}
    output:
      verdict:
        type: choice
        question: Is this a concern?
        criteria: {clear: Clear., concern: Concern.}
  second:
    input: {note: second}
    output:
      verdict:
        type: choice
        question: Is this a concern?
        criteria: {clear: Clear., concern: Concern.}
actions:
  - type: status
    input:
      result: '${{ decisions.first.outputs.verdict == ''concern'' && ''failure'' || ''success'' }}'
      message: 'Saved result: ${{ decisions.second.outputs.verdict }}'
";

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("fixtures/session.sql"))
        .unwrap();
    connection
        .execute(
            "UPDATE native_file_delivery SET parser_revision = ?1",
            [PARSER_REVISION],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO burn_check_activation VALUES ('retry-loop',?1,2,1,'revision')",
            [SCOPE],
        )
        .unwrap();
    connection.execute("INSERT INTO burn_check_runtime_definition VALUES ('retry-loop',?1,2,'revision','Saved typed status','available')", [SCOPE]).unwrap();
    connection
}

fn save_failure(connection: &Connection) {
    let check = compile(CHECK).unwrap();
    let requirements = check.question_requirements();
    let decisions = check
        .decisions
        .iter()
        .map(|declaration| {
            DecisionResult::new(
                declaration,
                &requirements,
                vec![
                    NamedAnswer::new(
                        "verdict",
                        QuestionResult::Choice(
                            ChoiceResult::new(
                                &declaration.outputs[0].question,
                                "concern",
                                None,
                                None,
                            )
                            .unwrap(),
                        ),
                    )
                    .unwrap(),
                ],
                1,
            )
            .unwrap()
        })
        .collect();
    let assessed = EvaluatedAssessment::new(&check.decisions, &requirements, decisions).unwrap();
    assert_eq!(assessed.decisions().len(), 2);
    let plan = plan_status_actions(&check.actions, &json!({}), &assessed, None).unwrap();
    connection.execute(
        "INSERT INTO burn_check_assessment_history VALUES ('a1',2,'assessed',1,?1,123,'retry-loop',?2,?3,0)",
        params![serde_json::to_string(&plan).unwrap(), SCOPE, BRANCH],
    ).unwrap();
    connection.execute("INSERT INTO burn_check_current_status VALUES ('retry-loop',?1,2,?2,'failure','a1',1,0)", params![SCOPE, BRANCH]).unwrap();
}

#[test]
fn saved_multi_decision_message_survives_a_later_local_no_write() {
    let connection = database();
    save_failure(&connection);
    let skipped = compile("name: No write\non:\n  tool_result:\nactions:\n  - type: status\n    if: false\n    input: {result: success, message: Do not replace the saved message.}\n").unwrap();
    let plan = plan_status_actions(
        &skipped.actions,
        &json!({}),
        &EvaluatedAssessment::local(),
        Some(StatusResult::Failure),
    )
    .unwrap();
    assert_eq!(plan.current_status, Some(StatusResult::Failure));
    let saved = serde_json::to_string(&plan).unwrap();
    assert!(status_message(Some(&saved), "failure").is_err());
    connection.execute(
        "INSERT INTO burn_check_assessment_history VALUES ('a2',2,'assessed',1,?1,456,'retry-loop',?2,?3,1)",
        params![saved, SCOPE, BRANCH],
    ).unwrap();
    let before = connection.total_changes();
    let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
    assert_eq!(checks[0].status, Some(CheckStatus::Failure));
    assert_eq!(checks[0].message.as_deref(), Some("Saved result: concern"));
    assert_eq!(checks[0].assessed_at_epoch, Some(123));
    assert_eq!(checks[0].eligibility, Eligibility::Ready);
    assert_eq!(connection.total_changes(), before);
}

#[test]
fn mismatched_status_history_is_an_error() {
    for change in [
        "UPDATE burn_check_assessment_history SET check_id = 'other'",
        "UPDATE burn_check_assessment_history SET activation_scope_key = '{}'",
        "UPDATE burn_check_assessment_history SET generation = 3",
        "UPDATE burn_check_assessment_history SET branch_key = '[\"native\",\"claude-code\",\"other\"]'",
        "UPDATE burn_check_assessment_history SET trigger_position = 1",
        "DELETE FROM burn_check_assessment_history",
    ] {
        let connection = database();
        save_failure(&connection);
        connection.execute_batch(change).unwrap();
        assert!(
            read_session_checks(&connection, "native", "claude-code", "s1").is_err(),
            "{change}"
        );
    }
}

#[test]
fn invalid_history_withholds_a_saved_status() {
    let connection = database();
    save_failure(&connection);
    connection
        .execute(
            "UPDATE burn_check_assessment_history SET evidence_valid = 0",
            [],
        )
        .unwrap();
    let checks = read_session_checks(&connection, "native", "claude-code", "s1").unwrap();
    assert_eq!(checks[0].status, None);
    assert_eq!(checks[0].message, None);
    assert_eq!(checks[0].assessed_at_epoch, None);
}

#[test]
fn malformed_actions_cannot_hide_behind_a_valid_write() {
    let applied =
        json!({"index": 0, "outcome": {"Applied": {"result": "Failure", "message": "Saved."}}});
    for outcomes in [
        json!([{"index": 0, "outcome": {"Applied": {"result": "Failure", "message": 7}}}]),
        json!([{"index": 0, "outcome": {"Applied": {"result": "Success", "message": "Wrong status."}}}]),
        json!([{"index": 0, "outcome": {"Applied": {"result": "Failure", "message": " "}}}]),
        json!([{"index": 0, "outcome": {"Applied": {"result": "Failure", "message": "x".repeat(4097)}}}]),
        json!([applied.clone(), {"index": 1, "outcome": "Unknown"}]),
        json!([{"index": 0, "outcome": "Unknown"}, {"index": 1, "outcome": {"Applied": {"result": "Failure", "message": "Saved."}}}]),
        json!([{"index": 1, "outcome": {"Applied": {"result": "Failure", "message": "Saved."}}}]),
    ] {
        let plan = json!({"current_status": "Failure", "outcomes": outcomes}).to_string();
        assert!(status_message(Some(&plan), "failure").is_err());
    }
    let valid = json!({"current_status": "Failure", "outcomes": [applied,
        {"index": 1, "outcome": "SkippedInputUnavailable"},
        {"index": 2, "outcome": {"Failed": {"code": "RuntimeInvalidContext", "message": "Invalid input."}}}
    ]}).to_string();
    assert_eq!(
        status_message(Some(&valid), "failure").unwrap().as_deref(),
        Some("Saved.")
    );
}
