use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;

use super::model::{
    CheckAssessment, CheckAssessmentSource, ChecksResponse, EvidenceRecord,
    PersistedAssessmentMetadata, SessionIdentity,
};

pub(crate) fn read_checks_from_paths(
    session_id: &str,
    paths: &[std::path::PathBuf],
    busy_timeout: Duration,
) -> Result<ChecksResponse, String> {
    if paths.is_empty() {
        return Err("no Antiburn database was found in the platform data directory".to_owned());
    }

    let mut match_found = None;
    for path in paths {
        let connection = open_read_only(path, busy_timeout)?;
        let identities = session_identities(&connection, session_id)?;
        if identities.len() > 1 {
            return Err(format!(
                "session ID {session_id:?} matches multiple Antiburn session identities"
            ));
        }
        let Some(identity) = identities.into_iter().next() else {
            continue;
        };
        if match_found.is_some() {
            return Err(format!(
                "session ID {session_id:?} matches more than one Antiburn database"
            ));
        }
        match_found = Some((identity, connection));
    }

    let Some((identity, mut connection)) = match_found else {
        return Err(format!(
            "session ID {session_id:?} was not found in the Antiburn database"
        ));
    };

    let connection = connection
        .transaction()
        .map_err(|error| format!("could not start checks read: {error}"))?;
    let evidence = read_evidence(&connection, &identity)?;
    let mut checks = read_assessments(&connection, &identity)?;
    checks.extend(super::evidence::derive_badges(&evidence)?);
    let has_yaml_registry: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'burn_check_runtime_definition')",
        [], |row| row.get(0),
    ).map_err(|error| format!("could not inspect check projection schema: {error}"))?;
    if has_yaml_registry {
        for check in antiburn_checks_store::read_session_checks(
            &connection,
            &identity.environment_key,
            &identity.agent,
            &identity.session_id,
        )
        .map_err(|error| format!("could not read predefined checks: {error}"))?
        {
            let status = match check.status {
                Some(antiburn_checks_store::CheckStatus::Failure) if check.message.is_some() => {
                    "finding"
                }
                Some(antiburn_checks_store::CheckStatus::Success) => "clean",
                _ => "not_assessed",
            };
            checks.push(CheckAssessment {
                source: CheckAssessmentSource::PredefinedYaml,
                check_id: check.check_id,
                status: status.into(),
                persisted: None,
                result: Some(
                    serde_json::json!({"id":check.id, "name":check.name, "message":check.message}),
                ),
            });
        }
    }
    connection
        .commit()
        .map_err(|error| format!("could not finish checks read: {error}"))?;
    Ok(ChecksResponse {
        session_id: session_id.to_owned(),
        identity,
        freshness_evaluated: false,
        evidence,
        checks,
    })
}

pub(crate) fn open_read_only(path: &Path, busy_timeout: Duration) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!(
            "could not open Antiburn database at {} for read-only access: {error}",
            path.display()
        )
    })?;
    connection
        .busy_timeout(busy_timeout)
        .map_err(|error| format!("could not set the Antiburn database busy timeout: {error}"))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| format!("could not enable read-only query mode: {error}"))?;
    Ok(connection)
}

pub(crate) fn session_identities(
    connection: &Connection,
    session_id: &str,
) -> Result<Vec<SessionIdentity>, String> {
    let mut statement = connection
        .prepare(
            "SELECT environment_key, agent, session_id
               FROM session
              WHERE session_id = ?1
              ORDER BY environment_key, agent",
        )
        .map_err(|error| format!("could not query Antiburn sessions: {error}"))?;
    let rows = statement
        .query_map([session_id], |row| {
            Ok(SessionIdentity {
                environment_key: row.get(0)?,
                agent: row.get(1)?,
                session_id: row.get(2)?,
            })
        })
        .map_err(|error| format!("could not query Antiburn sessions: {error}"))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| format!("could not read Antiburn session identities: {error}"))
}

fn read_evidence(
    connection: &Connection,
    identity: &SessionIdentity,
) -> Result<EvidenceRecord, String> {
    let diagnostics_column =
        if table_has_column(connection, "session_evidence", "diagnostics_json")? {
            "diagnostics_json"
        } else {
            "NULL AS diagnostics_json"
        };
    let query = format!(
        "SELECT status, analyzed_generation, processed_fingerprint,
                parser_revision, analyzer_revision, evidence_schema_revision,
                evidence_json, {diagnostics_column}
           FROM session_evidence
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3"
    );
    let row = connection
        .query_row(
            &query,
            params![
                identity.environment_key,
                identity.agent,
                identity.session_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("could not read Antiburn session evidence: {error}"))?;

    let Some((
        status,
        analyzed_generation,
        processed_fingerprint,
        parser_revision,
        analyzer_revision,
        evidence_schema_revision,
        evidence_json,
        diagnostics_json,
    )) = row
    else {
        return Ok(EvidenceRecord::missing());
    };

    let published_fence = if table_has_column(connection, "session_evidence", "published_fence")? {
        connection
            .query_row(
                "SELECT published_fence FROM session_evidence
                  WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    identity.environment_key,
                    identity.agent,
                    identity.session_id
                ],
                |row| row.get(0),
            )
            .map_err(|error| format!("could not read the evidence publication fence: {error}"))?
    } else {
        None
    };

    Ok(EvidenceRecord {
        status,
        analyzed_generation,
        processed_fingerprint,
        parser_revision,
        analyzer_revision,
        evidence_schema_revision,
        published_fence,
        data: parse_optional_json("session_evidence.evidence_json", evidence_json)?,
        diagnostics: parse_optional_json("session_evidence.diagnostics_json", diagnostics_json)?,
    })
}

fn read_assessments(
    connection: &Connection,
    identity: &SessionIdentity,
) -> Result<Vec<CheckAssessment>, String> {
    let mut statement = connection
        .prepare(
            "SELECT check_id, status, input_revision, evaluator_revision,
                    result_revision, incarnation, boundary_generation,
                    boundary_activity_cursor, boundary_at_epoch, source_generation,
                    source_fingerprint, published_fence, request_count,
                    created_at_epoch, updated_at_epoch, last_error_category,
                    progress_json, result_json
               FROM burn_check_assessment
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
              ORDER BY check_id",
        )
        .map_err(|error| format!("could not query Antiburn check assessments: {error}"))?;
    let rows = statement
        .query_map(
            params![
                identity.environment_key,
                identity.agent,
                identity.session_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, i64>(14)?,
                    row.get::<_, Option<String>>(15)?,
                    row.get::<_, String>(16)?,
                    row.get::<_, Option<String>>(17)?,
                ))
            },
        )
        .map_err(|error| format!("could not query Antiburn check assessments: {error}"))?;

    let mut assessments = Vec::new();
    for row in rows {
        let (
            check_id,
            status,
            input_revision,
            evaluator_revision,
            result_revision,
            incarnation,
            boundary_generation,
            boundary_activity_cursor,
            boundary_at_epoch,
            source_generation,
            source_fingerprint,
            published_fence,
            request_count,
            created_at_epoch,
            updated_at_epoch,
            last_error_category,
            progress_json,
            result_json,
        ) = row.map_err(|error| format!("could not read Antiburn check assessment: {error}"))?;
        let progress = parse_json(
            "burn_check_assessment.progress_json",
            &check_id,
            &progress_json,
        )?;
        let result = parse_optional_json("burn_check_assessment.result_json", result_json)?;
        let result_matches_input_revision =
            CheckAssessment::result_matches_input_revision(&input_revision, &result_revision);
        assessments.push(CheckAssessment {
            source: CheckAssessmentSource::BurnCheckAssessment,
            check_id,
            status,
            persisted: Some(Box::new(PersistedAssessmentMetadata {
                input_revision,
                evaluator_revision,
                result_revision,
                result_matches_input_revision,
                incarnation,
                boundary_generation,
                boundary_activity_cursor,
                boundary_at_epoch,
                source_generation,
                source_fingerprint,
                published_fence,
                request_count,
                created_at_epoch,
                updated_at_epoch,
                last_error_category,
                progress,
            })),
            result,
        });
    }
    Ok(assessments)
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| format!("could not inspect Antiburn database schema: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| format!("could not inspect Antiburn database schema: {error}"))?;
    for row in rows {
        if row.map_err(|error| format!("could not inspect Antiburn database schema: {error}"))?
            == column
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse_optional_json(field: &str, value: Option<String>) -> Result<Option<Value>, String> {
    value
        .map(|json| {
            serde_json::from_str(&json)
                .map_err(|error| format!("invalid {field} JSON in the Antiburn database: {error}"))
        })
        .transpose()
}

fn parse_json(field: &str, check_id: &str, json: &str) -> Result<Value, String> {
    serde_json::from_str(json).map_err(|error| {
        format!("invalid {field} JSON for check {check_id:?} in the Antiburn database: {error}")
    })
}

#[cfg(test)]
mod tests {
    use std::{
        path::{Path, PathBuf},
        time::Duration,
    };

    use antiburn_local::{
        analysis::{
            ContextEvidence, EvidenceSource, EvidenceValue, SessionEvidenceAccumulator,
            SourceCapabilities, SourceKind, TurnFacts,
        },
        insights::ReportCatalogs,
    };
    use rusqlite::{Connection, params};
    use serde_json::json;
    use tempfile::tempdir;

    use super::{open_read_only, read_checks_from_paths as read_with_timeout};
    use crate::checks::model::CheckAssessmentSource;

    fn read_checks_from_paths(
        session_id: &str,
        paths: &[PathBuf],
    ) -> Result<super::ChecksResponse, String> {
        read_with_timeout(session_id, paths, Duration::from_secs(5))
    }

    fn create_database(path: &Path) -> Connection {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                    environment_key TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    PRIMARY KEY (environment_key, agent, session_id)
                 );
                 CREATE TABLE session_evidence (
                    environment_key TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    analyzed_generation INTEGER,
                    processed_fingerprint TEXT,
                    parser_revision INTEGER,
                    analyzer_revision INTEGER,
                    evidence_schema_revision INTEGER,
                    evidence_json TEXT,
                    diagnostics_json TEXT,
                    published_fence INTEGER,
                    PRIMARY KEY (environment_key, agent, session_id)
                 );
                 CREATE TABLE burn_check_assessment (
                    environment_key TEXT NOT NULL,
                    agent TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    check_id TEXT NOT NULL,
                    incarnation INTEGER NOT NULL,
                    boundary_generation INTEGER,
                    boundary_activity_cursor TEXT,
                    boundary_at_epoch INTEGER,
                    input_revision TEXT,
                    evaluator_revision TEXT,
                    source_generation INTEGER,
                    source_fingerprint TEXT,
                    published_fence INTEGER,
                    status TEXT NOT NULL,
                    progress_json TEXT NOT NULL,
                    result_json TEXT,
                    result_revision TEXT,
                    request_count INTEGER NOT NULL,
                    created_at_epoch INTEGER NOT NULL,
                    updated_at_epoch INTEGER NOT NULL,
                    last_error_category TEXT,
                    PRIMARY KEY (environment_key, agent, session_id, check_id)
                 );",
            )
            .unwrap();
        connection
    }

    fn session_evidence_json(agent: &str, session_id: &str) -> String {
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        evidence.context = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: ReportCatalogs::default().depth_cap_tokens + 1,
            top_depth_examples: Vec::new(),
        });
        serde_json::to_string(&evidence).unwrap()
    }

    fn insert_session(connection: &Connection, environment: &str, agent: &str, id: &str) {
        connection
            .execute(
                "INSERT INTO session (environment_key, agent, session_id) VALUES (?1, ?2, ?3)",
                params![environment, agent, id],
            )
            .unwrap();
    }

    fn database_path(directory: &Path, name: &str) -> PathBuf {
        directory.join(name)
    }

    #[test]
    fn reads_evidence_and_check_results_as_separate_sources() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        insert_session(&connection, "native", "pi", "session-a");
        connection
            .execute(
                "INSERT INTO session_evidence (
                    environment_key, agent, session_id, status, analyzed_generation,
                    processed_fingerprint, parser_revision, analyzer_revision,
                    evidence_schema_revision, evidence_json, diagnostics_json, published_fence
                 ) VALUES ('native', 'pi', 'session-a', 'ready', 4, 'm:s', 1, 2, 3,
                           ?1, '{\"warnings\":[]}', 8)",
                [session_evidence_json("claude", "session-a")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO burn_check_assessment (
                    environment_key, agent, session_id, check_id, incarnation,
                    boundary_generation, boundary_activity_cursor, boundary_at_epoch,
                    input_revision, evaluator_revision, source_generation, source_fingerprint,
                    published_fence, status, progress_json, result_json, result_revision,
                    request_count, created_at_epoch, updated_at_epoch, last_error_category
                 ) VALUES ('native', 'pi', 'session-a', 'sessions_over_depth', 1,
                           2, 'cursor', 10, 'input-1', 'eval-1', 4, 'm:s', 8,
                           'completed', '{\"done\":1}', '{\"findings\":[\"x\"]}',
                           'input-1', 1, 11, 12, NULL)",
                [],
            )
            .unwrap();
        drop(connection);

        let response = read_checks_from_paths("session-a", std::slice::from_ref(&path)).unwrap();
        assert_eq!(response.identity.environment_key, "native");
        assert_eq!(response.identity.agent, "pi");
        assert_eq!(response.evidence.status, "ready");
        assert_eq!(
            response.evidence.data.as_ref().unwrap()["identity"]["sessionId"],
            "session-a"
        );
        assert_eq!(response.evidence.diagnostics, Some(json!({"warnings":[]})));
        assert_eq!(response.evidence.published_fence, Some(8));
        assert_eq!(response.checks.len(), 7);
        assert_eq!(
            response.checks[0].source,
            CheckAssessmentSource::BurnCheckAssessment
        );
        assert_eq!(response.checks[0].check_id, "sessions_over_depth");
        assert_eq!(response.checks[0].status, "completed");
        assert_eq!(response.checks[0].result, Some(json!({"findings":["x"]})));
        assert_eq!(
            response.checks[0]
                .persisted
                .as_ref()
                .unwrap()
                .result_matches_input_revision,
            Some(true)
        );
        assert_eq!(
            response.checks[1].source,
            CheckAssessmentSource::SessionEvidenceBadge
        );
        assert_eq!(response.checks[1].check_id, "sessions_over_depth");
        assert!(
            response.checks[1..]
                .iter()
                .all(|check| { check.source == CheckAssessmentSource::SessionEvidenceBadge })
        );
        assert!(!response.freshness_evaluated);
    }

    #[test]
    fn scopes_lookup_to_the_explicit_session_id() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        insert_session(&connection, "native", "pi", "other-session");
        insert_session(&connection, "native", "codex", "requested-session");
        drop(connection);

        let response = read_checks_from_paths("requested-session", &[path]).unwrap();
        assert_eq!(response.session_id, "requested-session");
        assert_eq!(response.identity.agent, "codex");
    }

    #[test]
    fn rejects_ambiguous_session_identity_in_one_database() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        insert_session(&connection, "native", "pi", "duplicate-id");
        insert_session(&connection, "wsl:ubuntu", "pi", "duplicate-id");
        drop(connection);

        let error = read_checks_from_paths("duplicate-id", &[path]).unwrap_err();
        assert!(error.contains("multiple Antiburn session identities"));
    }

    #[test]
    fn rejects_a_session_id_found_in_more_than_one_database() {
        let temp = tempdir().unwrap();
        let first = database_path(temp.path(), "first.sqlite3");
        let second = database_path(temp.path(), "second.sqlite3");
        let first_connection = create_database(&first);
        insert_session(&first_connection, "native", "pi", "duplicate-id");
        drop(first_connection);
        let second_connection = create_database(&second);
        insert_session(&second_connection, "native", "pi", "duplicate-id");
        drop(second_connection);

        let error = read_checks_from_paths("duplicate-id", &[first, second]).unwrap_err();
        assert!(error.contains("more than one Antiburn database"));
    }

    #[test]
    fn missing_database_and_session_return_errors() {
        assert!(
            read_checks_from_paths("session-a", &[])
                .unwrap_err()
                .contains("no Antiburn database")
        );

        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        drop(create_database(&path));
        assert!(
            read_checks_from_paths("missing", &[path])
                .unwrap_err()
                .contains("was not found")
        );
    }

    #[test]
    fn reads_evidence_when_the_diagnostics_column_is_absent() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        connection
            .execute_batch("ALTER TABLE session_evidence DROP COLUMN diagnostics_json;")
            .unwrap();
        insert_session(&connection, "native", "pi", "session-a");
        connection
            .execute(
                "INSERT INTO session_evidence (
                    environment_key, agent, session_id, status, evidence_json
                 ) VALUES ('native', 'pi', 'session-a', 'failed', NULL)",
                [],
            )
            .unwrap();
        drop(connection);

        let response = read_checks_from_paths("session-a", &[path]).unwrap();
        assert_eq!(response.evidence.status, "failed");
        assert!(response.evidence.diagnostics.is_none());
    }

    #[test]
    fn missing_evidence_and_assessments_are_not_clean_results() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        insert_session(&connection, "native", "pi", "session-a");
        drop(connection);

        let response = read_checks_from_paths("session-a", &[path]).unwrap();
        assert_eq!(response.evidence.status, "missing");
        assert!(response.evidence.data.is_none());
        assert!(response.checks.is_empty());
        assert!(!response.freshness_evaluated);
    }

    #[test]
    fn rejects_malformed_json_in_stored_sources() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        insert_session(&connection, "native", "pi", "session-a");
        connection
            .execute(
                "INSERT INTO session_evidence (
                    environment_key, agent, session_id, status, evidence_json
                 ) VALUES ('native', 'pi', 'session-a', 'ready', '{bad json}')",
                [],
            )
            .unwrap();
        drop(connection);

        let error = read_checks_from_paths("session-a", &[path]).unwrap_err();
        assert!(error.contains("invalid session_evidence.evidence_json JSON"));
    }

    #[test]
    fn read_only_connection_rejects_database_writes() {
        let temp = tempdir().unwrap();
        let path = database_path(temp.path(), "antiburn.sqlite3");
        let connection = create_database(&path);
        insert_session(&connection, "native", "pi", "session-a");
        drop(connection);

        let connection = open_read_only(&path, Duration::from_secs(5)).unwrap();
        let query_only: bool = connection
            .pragma_query_value(None, "query_only", |row| row.get(0))
            .unwrap();
        assert!(query_only);
        assert!(
            connection
                .execute(
                    "UPDATE session SET agent = 'changed' WHERE session_id = 'session-a'",
                    [],
                )
                .is_err()
        );
    }
}
