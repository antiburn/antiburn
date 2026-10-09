pub(crate) fn current_evidence(evidence: &str, session: &str) -> String {
    format!(
        "NOT ({evidence}.analyzed_generation IS NOT {session}.source_generation)
         AND NOT ({evidence}.parser_revision IS NOT :parser_revision)
         AND NOT ({evidence}.analyzer_revision IS NOT :analyzer_revision)
         AND NOT ({evidence}.evidence_schema_revision IS NOT :evidence_schema_revision)"
    )
}

pub(crate) fn current_analysis(analysis: &str, session: &str) -> String {
    format!(
        "NOT ({analysis}.analyzed_generation IS NOT {session}.source_generation)
         AND NOT ({analysis}.parser_revision IS NOT :parser_revision)
         AND NOT ({analysis}.analyzer_revision IS NOT :analyzer_revision)
         AND NOT ({analysis}.metrics_schema_revision IS NOT :metrics_schema_revision)"
    )
}

pub(crate) fn captured_evidence_revisions(evidence: &str) -> String {
    format!(
        "{evidence}.parser_revision = :captured_parser_revision
         AND {evidence}.analyzer_revision = :captured_analyzer_revision
         AND {evidence}.evidence_schema_revision = :captured_evidence_schema_revision"
    )
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, named_params};

    use super::{captured_evidence_revisions, current_analysis, current_evidence};

    #[test]
    fn current_evidence_revision_bindings_reject_stale_and_null_values() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (source_generation INTEGER);
                 CREATE TABLE evidence (
                     analyzed_generation INTEGER, parser_revision INTEGER,
                     analyzer_revision INTEGER, evidence_schema_revision INTEGER
                 );
                 INSERT INTO session VALUES (3);
                 INSERT INTO evidence VALUES (3, 11, 12, 13);",
            )
            .unwrap();
        let predicate = current_evidence("evidence", "session");
        let sql = format!("SELECT {predicate} FROM evidence, session");
        let mut statement = connection.prepare(&sql).unwrap();
        assert_eq!(statement.parameter_count(), 3);
        assert_eq!(
            statement.parameter_index(":parser_revision").unwrap(),
            Some(1)
        );
        assert_eq!(
            statement.parameter_index(":analyzer_revision").unwrap(),
            Some(2)
        );
        assert_eq!(
            statement
                .parameter_index(":evidence_schema_revision")
                .unwrap(),
            Some(3)
        );
        let bindings = named_params![
            ":parser_revision": 11,
            ":analyzer_revision": 12,
            ":evidence_schema_revision": 13,
        ];
        assert!(
            statement
                .query_row(bindings, |row| row.get::<_, bool>(0))
                .unwrap()
        );

        connection
            .execute("UPDATE evidence SET parser_revision = 10", [])
            .unwrap();
        assert!(
            !statement
                .query_row(
                    named_params![
                        ":parser_revision": 11,
                        ":analyzer_revision": 12,
                        ":evidence_schema_revision": 13,
                    ],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
        connection
            .execute("UPDATE evidence SET parser_revision = NULL", [])
            .unwrap();
        assert!(
            !statement
                .query_row(
                    named_params![
                        ":parser_revision": 11,
                        ":analyzer_revision": 12,
                        ":evidence_schema_revision": 13,
                    ],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
    }

    #[test]
    fn current_analysis_uses_its_own_metrics_revision_binding() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (source_generation INTEGER);
                 CREATE TABLE analysis (
                     analyzed_generation INTEGER, parser_revision INTEGER,
                     analyzer_revision INTEGER, metrics_schema_revision INTEGER
                 );
                 INSERT INTO session VALUES (3);
                 INSERT INTO analysis VALUES (3, 11, 12, 14);",
            )
            .unwrap();
        let sql = format!(
            "SELECT {} FROM analysis, session",
            current_analysis("analysis", "session")
        );
        let mut statement = connection.prepare(&sql).unwrap();
        assert_eq!(statement.parameter_count(), 3);
        assert!(
            statement
                .query_row(
                    named_params![
                        ":parser_revision": 11,
                        ":analyzer_revision": 12,
                        ":metrics_schema_revision": 14,
                    ],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
        assert!(
            !statement
                .query_row(
                    named_params![
                        ":parser_revision": 11,
                        ":analyzer_revision": 12,
                        ":metrics_schema_revision": 15,
                    ],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
    }

    #[test]
    fn captured_revision_bindings_are_distinct_from_runtime_bindings() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE evidence (
                     parser_revision INTEGER, analyzer_revision INTEGER,
                     evidence_schema_revision INTEGER
                 );
                 INSERT INTO evidence VALUES (11, 12, 13);",
            )
            .unwrap();
        let sql = format!(
            "SELECT {} FROM evidence",
            captured_evidence_revisions("evidence")
        );
        let mut statement = connection.prepare(&sql).unwrap();
        assert_eq!(statement.parameter_count(), 3);
        assert!(
            statement
                .query_row(
                    named_params![
                        ":captured_parser_revision": 11,
                        ":captured_analyzer_revision": 12,
                        ":captured_evidence_schema_revision": 13,
                    ],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
        assert!(
            !statement
                .query_row(
                    named_params![
                        ":captured_parser_revision": 10,
                        ":captured_analyzer_revision": 12,
                        ":captured_evidence_schema_revision": 13,
                    ],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap()
        );
    }
}
