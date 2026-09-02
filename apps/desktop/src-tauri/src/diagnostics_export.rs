//! The privacy-scoped diagnostics export.
//!
//! The document carries the derived evidence needed to explain analysis and
//! badge states. It excludes every transcript body and every location or title
//! that could identify the reader's work. Turn rows become per-scope counts
//! before they leave SQLite. The export never copies a turn row or content.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::types::Type;
use rusqlite::{Row, Transaction};
use serde::Serialize;

use crate::store::open_read_only;

const EXPORT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EXPORTED_SESSIONS: u64 = 500;

pub const FORMAT: &str = "antiburn.diagnostics-export";
pub const FORMAT_VERSION: u32 = 1;
pub const CONTENT_NOTICE: &str = concat!(
    "This export contains derived diagnostics for up to 500 recent sessions in antiburn's local index. ",
    "It includes opaque session identifiers, agent names, activity times, model and setting ",
    "labels, tool and skill names and descriptions present in derived evidence, aggregate ",
    "turn counts, evidence lifecycle state, revisions, and errors. It excludes transcript ",
    "bodies, message text, tool arguments and results, file contents, session titles, source ",
    "paths, working directories, repository names, provider-account keys, analytics ",
    "identifiers, and turn_content. Review the file before sharing it."
);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsExport {
    format: &'static str,
    format_version: u32,
    exported_at: String,
    app_version: String,
    content_notice: &'static str,
    database_schema_version: i64,
    current_revisions: CurrentRevisions,
    scope: ExportScope,
    sessions: Vec<SessionDiagnostics>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentRevisions {
    parser: i64,
    analyzer: i64,
    metrics_schema: i64,
    evidence_schema: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportScope {
    indexed_sessions: u64,
    exported_sessions: u64,
    session_limit: u64,
    omitted_sessions: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDiagnostics {
    environment_key: String,
    agent: String,
    session_id: String,
    source_kind: String,
    updated_at_epoch: Option<i64>,
    source_generation: i64,
    evidence: Option<EvidenceDiagnostics>,
    turn_summary: TurnSummary,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceDiagnostics {
    status: String,
    analyzed_generation: Option<i64>,
    parser_revision: Option<i64>,
    analyzer_revision: Option<i64>,
    evidence_schema_revision: Option<i64>,
    evidence_json: Option<serde_json::Value>,
    evidence_json_error: Option<String>,
    retry_count: i64,
    analyzed_at_epoch: Option<i64>,
    last_error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnSummary {
    main: TurnScopeSummary,
    delegated: TurnScopeSummary,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnScopeSummary {
    turns: u64,
    assistant_turns: u64,
    assistant_turns_with_model: u64,
    assistant_turns_with_effort: u64,
    assistant_turns_with_speed: u64,
    timestamped_assistant_turns: u64,
}

type SessionIdentity = (String, String, String);

/// Read one pinned database snapshot and build the diagnostics document.
pub fn build(data_dir: &Path, app_version: String) -> Result<DiagnosticsExport> {
    let mut connection = open_read_only(data_dir, EXPORT_BUSY_TIMEOUT)?;
    let transaction = connection.transaction()?;
    build_from_transaction(&transaction, app_version, crate::store::now_rfc3339())
}

impl DiagnosticsExport {
    /// Serialize the document as one pretty-printed JSON file.
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("failed to serialize the diagnostics export")
    }
}

fn build_from_transaction(
    transaction: &Transaction<'_>,
    app_version: String,
    exported_at: String,
) -> Result<DiagnosticsExport> {
    let database_schema_version =
        transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let indexed_sessions =
        nonnegative_value(
            transaction.query_row("SELECT COUNT(*) FROM session", [], |row| row.get(0))?,
        )?;
    let turn_summaries = read_turn_summaries(transaction)?;
    let sessions = read_sessions(transaction, turn_summaries)?;
    let exported_sessions = u64::try_from(sessions.len())?;

    Ok(DiagnosticsExport {
        format: FORMAT,
        format_version: FORMAT_VERSION,
        exported_at,
        app_version,
        content_notice: CONTENT_NOTICE,
        database_schema_version,
        current_revisions: CurrentRevisions {
            parser: antiburn_local::analysis::PARSER_REVISION,
            analyzer: antiburn_local::analysis::ANALYZER_REVISION,
            metrics_schema: antiburn_local::analysis::METRICS_SCHEMA_REVISION,
            evidence_schema: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
        },
        scope: ExportScope {
            indexed_sessions,
            exported_sessions,
            session_limit: MAX_EXPORTED_SESSIONS,
            omitted_sessions: indexed_sessions.saturating_sub(exported_sessions),
        },
        sessions,
    })
}

fn read_turn_summaries(
    transaction: &Transaction<'_>,
) -> Result<BTreeMap<SessionIdentity, TurnSummary>> {
    let mut statement = transaction.prepare(
        "SELECT t.environment_key, t.agent, t.session_id,
                t.scope, COUNT(*),
                SUM(t.role = 'assistant'),
                SUM(t.role = 'assistant' AND t.model IS NOT NULL),
                SUM(t.role = 'assistant' AND t.effort IS NOT NULL),
                SUM(t.role = 'assistant' AND t.speed IS NOT NULL),
                SUM(t.role = 'assistant' AND t.ts_ms IS NOT NULL)
           FROM turn t
           JOIN session_evidence e
             ON e.environment_key = t.environment_key
            AND e.agent = t.agent
            AND e.session_id = t.session_id
            AND e.published_fence = t.claim_fence
           JOIN (
                SELECT environment_key, agent, session_id
                  FROM session
                 ORDER BY COALESCE(updated_at_epoch, 0) DESC,
                          environment_key, agent, session_id
                 LIMIT ?1
           ) selected
             ON selected.environment_key = t.environment_key
            AND selected.agent = t.agent
            AND selected.session_id = t.session_id
          WHERE t.scope IN ('main', 'delegated')
          GROUP BY t.environment_key, t.agent, t.session_id, t.scope
          ORDER BY t.environment_key, t.agent, t.session_id, t.scope",
    )?;
    let rows = statement.query_map([MAX_EXPORTED_SESSIONS], |row| {
        Ok((
            (
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ),
            row.get::<_, String>(3)?,
            TurnScopeSummary {
                turns: nonnegative_count(row, 4)?,
                assistant_turns: nonnegative_count(row, 5)?,
                assistant_turns_with_model: nonnegative_count(row, 6)?,
                assistant_turns_with_effort: nonnegative_count(row, 7)?,
                assistant_turns_with_speed: nonnegative_count(row, 8)?,
                timestamped_assistant_turns: nonnegative_count(row, 9)?,
            },
        ))
    })?;

    let mut by_session = BTreeMap::<SessionIdentity, TurnSummary>::new();
    for row in rows {
        let (identity, scope, summary) = row?;
        let session = by_session.entry(identity).or_default();
        match scope.as_str() {
            "main" => session.main = summary,
            "delegated" => session.delegated = summary,
            _ => {}
        }
    }
    Ok(by_session)
}

fn nonnegative_count(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let count = row.get::<_, i64>(index)?;
    u64::try_from(count).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Integer, Box::new(error))
    })
}

fn nonnegative_value(value: i64) -> Result<u64> {
    u64::try_from(value).context("SQLite returned a negative count")
}

fn read_sessions(
    transaction: &Transaction<'_>,
    mut turn_summaries: BTreeMap<SessionIdentity, TurnSummary>,
) -> Result<Vec<SessionDiagnostics>> {
    let mut statement = transaction.prepare(
        "SELECT s.environment_key, s.agent, s.session_id, s.source_kind,
                s.updated_at_epoch, s.source_generation,
                e.status, e.analyzed_generation,
                e.parser_revision, e.analyzer_revision, e.evidence_schema_revision,
                e.evidence_json, e.retry_count, e.analyzed_at_epoch, e.last_error
           FROM session s
           LEFT JOIN session_evidence e
             ON e.environment_key = s.environment_key
            AND e.agent = s.agent
            AND e.session_id = s.session_id
          ORDER BY COALESCE(s.updated_at_epoch, 0) DESC,
                   s.environment_key, s.agent, s.session_id
          LIMIT ?1",
    )?;
    let rows = statement.query_map([MAX_EXPORTED_SESSIONS], |row| {
        let identity = (
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        );
        let evidence = match row.get::<_, Option<String>>(6)? {
            Some(status) => {
                let (evidence_json, evidence_json_error) =
                    parse_evidence_json(row.get::<_, Option<String>>(11)?);
                Some(EvidenceDiagnostics {
                    status,
                    analyzed_generation: row.get(7)?,
                    parser_revision: row.get(8)?,
                    analyzer_revision: row.get(9)?,
                    evidence_schema_revision: row.get(10)?,
                    evidence_json,
                    evidence_json_error,
                    retry_count: row.get::<_, Option<i64>>(12)?.unwrap_or_default(),
                    analyzed_at_epoch: row.get(13)?,
                    last_error: row.get(14)?,
                })
            }
            None => None,
        };
        Ok((
            identity,
            row.get::<_, String>(3)?,
            row.get(4)?,
            row.get(5)?,
            evidence,
        ))
    })?;

    let mut sessions = Vec::new();
    for row in rows {
        let (identity, source_kind, updated_at_epoch, source_generation, evidence) = row?;
        sessions.push(SessionDiagnostics {
            environment_key: identity.0.clone(),
            agent: identity.1.clone(),
            session_id: identity.2.clone(),
            source_kind,
            updated_at_epoch,
            source_generation,
            evidence,
            turn_summary: turn_summaries.remove(&identity).unwrap_or_default(),
        });
    }
    Ok(sessions)
}

fn parse_evidence_json(
    evidence_json: Option<String>,
) -> (Option<serde_json::Value>, Option<String>) {
    let Some(evidence_json) = evidence_json else {
        return (None, None);
    };
    match serde_json::from_str(&evidence_json) {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use rusqlite::Connection;

    fn fixture_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("create the fixture database");
        connection
            .execute_batch(
                "PRAGMA user_version = 27;
                 CREATE TABLE session (
                     environment_key TEXT NOT NULL,
                     agent TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     source_kind TEXT NOT NULL,
                     source_label TEXT NOT NULL,
                     title TEXT,
                     cwd TEXT,
                     updated_at_epoch INTEGER,
                     source_generation INTEGER NOT NULL,
                     PRIMARY KEY (environment_key, agent, session_id)
                 );
                 CREATE TABLE session_evidence (
                     environment_key TEXT NOT NULL,
                     agent TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     status TEXT NOT NULL,
                     analyzed_generation INTEGER,
                     parser_revision INTEGER,
                     analyzer_revision INTEGER,
                     evidence_schema_revision INTEGER,
                     evidence_json TEXT,
                     retry_count INTEGER NOT NULL,
                     analyzed_at_epoch INTEGER,
                     last_error TEXT,
                     published_fence INTEGER,
                     PRIMARY KEY (environment_key, agent, session_id)
                 );
                 CREATE TABLE turn (
                     environment_key TEXT NOT NULL,
                     agent TEXT NOT NULL,
                     session_id TEXT NOT NULL,
                     claim_fence INTEGER NOT NULL,
                     scope TEXT NOT NULL,
                     role TEXT NOT NULL,
                     model TEXT,
                     effort TEXT,
                     speed TEXT,
                     ts_ms INTEGER
                 );
                 CREATE TABLE turn_content (
                     turn_rowid INTEGER NOT NULL,
                     part_index INTEGER NOT NULL,
                     content BLOB NOT NULL
                 );",
            )
            .expect("create the fixture schema");
        connection
    }

    #[test]
    fn the_export_contains_badge_diagnostics_without_private_session_content() {
        let mut connection = fixture_connection();
        connection
            .execute(
                "INSERT INTO session (
                     environment_key, agent, session_id, source_kind, source_label,
                     title, cwd, updated_at_epoch, source_generation
                 ) VALUES ('native', 'claude-code', 'session-123', 'file',
                           'PRIVATE-SOURCE-PATH', 'PRIVATE-TITLE', 'PRIVATE-CWD', 100, 4)",
                [],
            )
            .expect("insert the fixture session");
        connection
            .execute(
                "INSERT INTO session_evidence (
                     environment_key, agent, session_id, status, analyzed_generation,
                     parser_revision, analyzer_revision, evidence_schema_revision,
                     evidence_json, retry_count, analyzed_at_epoch, last_error, published_fence
                 ) VALUES ('native', 'claude-code', 'session-123', 'ready', 4,
                           1, 2, 3, '{\"coverage\":\"complete\"}', 0, 101, NULL, 7)",
                [],
            )
            .expect("insert the fixture evidence");
        connection
            .execute(
                "INSERT INTO turn (
                     environment_key, agent, session_id, claim_fence, scope, role,
                     model, effort, speed, ts_ms
                 ) VALUES ('native', 'claude-code', 'session-123', 7, 'main',
                           'assistant', 'claude-opus-4-1', NULL, 'standard', 100000)",
                [],
            )
            .expect("insert the fixture turn");
        connection
            .execute(
                "INSERT INTO turn_content VALUES (1, 0, 'PRIVATE-TRANSCRIPT-BODY')",
                [],
            )
            .expect("insert the private fixture content");

        let transaction = connection
            .transaction()
            .expect("start the fixture snapshot");
        let export = build_from_transaction(
            &transaction,
            "0.3.1".to_owned(),
            "2026-09-02T00:00:00Z".to_owned(),
        )
        .expect("build the fixture export");
        let json = export.to_json().expect("serialize the fixture export");
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("parse the fixture export");

        assert_eq!(value["format"], FORMAT);
        assert_eq!(value["databaseSchemaVersion"], 27);
        assert_eq!(value["sessions"][0]["evidence"]["status"], "ready");
        assert_eq!(
            value["sessions"][0]["evidence"]["evidenceJson"]["coverage"],
            "complete"
        );
        assert_eq!(value["scope"]["sessionLimit"], 500);
        assert_eq!(value["scope"]["exportedSessions"], 1);
        assert_eq!(
            value["sessions"][0]["turnSummary"]["main"]["assistantTurnsWithModel"],
            1
        );
        assert_eq!(
            value["sessions"][0]["turnSummary"]["main"]["assistantTurnsWithEffort"],
            0
        );
        assert_eq!(
            value["sessions"][0]["turnSummary"]["main"]["assistantTurnsWithSpeed"],
            1
        );
        assert_eq!(
            value["sessions"][0]["turnSummary"]["main"]["timestampedAssistantTurns"],
            1
        );
        for private_value in [
            "PRIVATE-SOURCE-PATH",
            "PRIVATE-TITLE",
            "PRIVATE-CWD",
            "PRIVATE-TRANSCRIPT-BODY",
        ] {
            assert!(!json.contains(private_value));
        }
    }

    #[test]
    fn malformed_evidence_is_named_without_copying_the_invalid_value() {
        let (value, error) = parse_evidence_json(Some("PRIVATE-INVALID-EVIDENCE{".to_owned()));

        assert!(value.is_none());
        assert!(error.is_some());
        assert!(
            !error
                .expect("malformed evidence must report an error")
                .contains("PRIVATE-INVALID-EVIDENCE")
        );
    }

    #[test]
    fn the_export_reads_the_current_migrated_schema() {
        let directory = tempfile::tempdir().expect("create the migrated fixture directory");
        let _store = Store::open(directory.path()).expect("create the migrated fixture store");

        let export = build(directory.path(), "0.3.1".to_owned())
            .expect("build from the migrated fixture store");
        let value = serde_json::to_value(export).expect("serialize the migrated export");

        assert_eq!(value["scope"]["indexedSessions"], 0);
        assert_eq!(value["sessions"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn the_export_bounds_sessions_to_the_most_recent_five_hundred() {
        let mut connection = fixture_connection();
        let transaction = connection
            .transaction()
            .expect("start the bounded fixture snapshot");
        for index in 0..=MAX_EXPORTED_SESSIONS {
            transaction
                .execute(
                    "INSERT INTO session (
                         environment_key, agent, session_id, source_kind, source_label,
                         updated_at_epoch, source_generation
                     ) VALUES ('native', 'claude-code', ?1, 'file', 'private-path', ?2, 1)",
                    rusqlite::params![format!("session-{index:03}"), index],
                )
                .expect("insert a bounded fixture session");
        }

        let export = build_from_transaction(
            &transaction,
            "0.3.1".to_owned(),
            "2026-09-02T00:00:00Z".to_owned(),
        )
        .expect("build the bounded fixture export");
        let value = serde_json::to_value(export).expect("serialize the bounded export");

        assert_eq!(value["scope"]["indexedSessions"], 501);
        assert_eq!(value["scope"]["exportedSessions"], 500);
        assert_eq!(value["scope"]["omittedSessions"], 1);
        assert_eq!(value["sessions"].as_array().map(Vec::len), Some(500));
        assert_eq!(value["sessions"][0]["sessionId"], "session-500");
        assert_eq!(value["sessions"][499]["sessionId"], "session-001");
    }
}
