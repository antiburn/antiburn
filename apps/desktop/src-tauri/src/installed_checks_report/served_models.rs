use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

const MAX_REPORTED_ATTEMPTS: usize = 256;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ServedModels {
    attempts: Vec<ServedModelAttempt>,
    truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServedModelAttempt {
    decision: Option<String>,
    attempt: u8,
    usage_known: bool,
    served_model: Option<String>,
}

pub(super) fn for_assessment(connection: &Connection, assessment: &str) -> Result<ServedModels> {
    let mut statement = connection.prepare(
        "SELECT a.decision_name, a.attempt_number, u.usage_known, u.served_model
           FROM burn_check_assessment_history h
           JOIN burn_check_evaluation_attempt a ON a.candidate_id = h.candidate_id
           JOIN burn_check_decision_usage u ON u.attempt_id = a.attempt_id
          WHERE h.assessment_id = ?1
          ORDER BY a.decision_name, a.attempt_number
          LIMIT ?2",
    )?;
    let rows = statement.query_map(
        rusqlite::params![assessment, MAX_REPORTED_ATTEMPTS + 1],
        |row| {
            Ok(ServedModelAttempt {
                decision: row.get(0)?,
                attempt: row.get(1)?,
                usage_known: row.get(2)?,
                served_model: row.get(3)?,
            })
        },
    )?;
    let mut attempts = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = attempts.len() > MAX_REPORTED_ATTEMPTS;
    attempts.truncate(MAX_REPORTED_ATTEMPTS);
    Ok(ServedModels {
        attempts,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE burn_check_assessment_history (assessment_id TEXT, candidate_id TEXT);
             CREATE TABLE burn_check_evaluation_attempt (candidate_id TEXT, attempt_id TEXT, decision_name TEXT, attempt_number INTEGER);
             CREATE TABLE burn_check_decision_usage (attempt_id TEXT, usage_known INTEGER, served_model TEXT);
             INSERT INTO burn_check_assessment_history VALUES ('writing', 'original'), ('later', 'new');
             INSERT INTO burn_check_evaluation_attempt VALUES
                 ('original', 'a1', 'review', 1), ('original', 'a2', 'review', 2),
                 ('original', 'b1', 'confirm', 1), ('new', 'c1', 'review', 1);
             INSERT INTO burn_check_decision_usage VALUES
                 ('a1', 1, 'jev-1.13.0'), ('a2', 1, 'jev-1.14.0'),
                 ('b1', 1, NULL), ('c1', 1, 'future-model');",
        ).unwrap();
        connection
    }

    #[test]
    fn observations_bind_to_the_status_writing_assessment_and_each_attempt() {
        let connection = fixture();
        let value = serde_json::to_value(for_assessment(&connection, "writing").unwrap()).unwrap();
        assert_eq!(
            value,
            json!({
                "attempts": [
                    {"decision":"confirm", "attempt":1, "usageKnown":true, "servedModel":null},
                    {"decision":"review", "attempt":1, "usageKnown":true, "servedModel":"jev-1.13.0"},
                    {"decision":"review", "attempt":2, "usageKnown":true, "servedModel":"jev-1.14.0"}
                ],
                "truncated":false
            })
        );
        let serialized = value.to_string();
        for unrelated in ["future-model", "original", "a1", "a2", "b1"] {
            assert!(!serialized.contains(unrelated));
        }
        assert_eq!(
            serde_json::to_value(for_assessment(&connection, "missing").unwrap()).unwrap(),
            json!({"attempts":[], "truncated":false})
        );
    }

    #[test]
    fn report_observations_have_an_explicit_row_bound() {
        let connection = fixture();
        for index in 0..MAX_REPORTED_ATTEMPTS {
            let id = format!("extra-{index}");
            connection
                .execute(
                    "INSERT INTO burn_check_evaluation_attempt VALUES ('original', ?1, ?1, 1)",
                    [&id],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO burn_check_decision_usage VALUES (?1, 0, NULL)",
                    [&id],
                )
                .unwrap();
        }
        let result = for_assessment(&connection, "writing").unwrap();
        assert_eq!(result.attempts.len(), MAX_REPORTED_ATTEMPTS);
        assert!(result.truncated);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_decision_usage",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            MAX_REPORTED_ATTEMPTS + 4
        );
    }
}
