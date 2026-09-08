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
use crate::store::provider_limit::{self, LimitFactorDiagnostics};

const EXPORT_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_EXPORTED_SESSIONS: u64 = 500;

pub const FORMAT: &str = "antiburn.diagnostics-export";
/// Bumped for the added `limitFactors` section: a consumer parsing an older
/// export would otherwise have no signal that this section can be absent.
pub const FORMAT_VERSION: u32 = 2;
pub const CONTENT_NOTICE: &str = concat!(
    "This export contains derived diagnostics for up to 500 recent sessions in antiburn's local index. ",
    "It includes opaque session identifiers, agent names, activity times, model and setting ",
    "labels, tool and skill names and descriptions present in derived evidence, aggregate ",
    "turn counts, evidence lifecycle state, revisions, and errors, plus one entry per learned ",
    "session-limit factor with its dollars-per-percent value, sample and point counts, plan, ",
    "and latest residual. It excludes transcript bodies, message text, tool arguments and ",
    "results, file contents, session titles, source paths, working directories, repository ",
    "names, provider-account keys, analytics identifiers, and turn_content. Review the file ",
    "before sharing it."
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
    limit_factors: Vec<LimitFactorEntry>,
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

/// One learned session-limit factor, grouped by `(provider, lane)`.
///
/// `account` names a stable per-export ordinal such as `"account 2"`, present
/// only when the provider has more than one account for this lane. It is
/// never the account key: the export's [`CONTENT_NOTICE`] promises that key
/// stays out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LimitFactorEntry {
    provider: String,
    lane: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    usd_per_percent: f64,
    method: String,
    sample_count: i64,
    point_count: i64,
    delta_sample_count: i64,
    unattributed_sample_count: i64,
    plan: Option<String>,
    plan_tier: Option<String>,
    residual: Option<LimitResidualEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LimitResidualEntry {
    meter_percent: f64,
    estimated_percent: f64,
    computed_at_epoch: i64,
}

/// Read one pinned database snapshot and build the diagnostics document.
pub fn build(data_dir: &Path, app_version: String) -> Result<DiagnosticsExport> {
    let mut connection = open_read_only(data_dir, EXPORT_BUSY_TIMEOUT)?;
    let transaction = connection.transaction()?;
    build_from_transaction(
        &transaction,
        app_version,
        crate::store::now_rfc3339(),
        crate::scan::unix_now(),
    )
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
    now_epoch: i64,
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
    let limit_factors = read_limit_factors(transaction, now_epoch)?;

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
        limit_factors,
    })
}

/// Group [`LimitFactorDiagnostics`] rows by `(provider, lane)` and replace
/// each row's opaque account key with a stable per-export ordinal.
///
/// A lane with exactly one account carries no `account` field at all: there
/// is nothing to disambiguate. A lane with more than one gets `"account 1"`,
/// `"account 2"`, and so on, ordered by the account key so the numbering does
/// not reshuffle between reads of the same database.
fn read_limit_factors(
    transaction: &Transaction<'_>,
    now_epoch: i64,
) -> Result<Vec<LimitFactorEntry>> {
    let diagnostics = provider_limit::limit_factor_diagnostics_in(transaction, now_epoch)?;
    let mut grouped: BTreeMap<(String, String), Vec<LimitFactorDiagnostics>> = BTreeMap::new();
    for diagnostic in diagnostics {
        grouped
            .entry((diagnostic.provider.clone(), diagnostic.lane.clone()))
            .or_default()
            .push(diagnostic);
    }

    let mut entries = Vec::new();
    for ((provider, lane), mut group) in grouped {
        group.sort_by(|left, right| left.account_key.cmp(&right.account_key));
        let multiple_accounts = group.len() > 1;
        for (index, diagnostic) in group.into_iter().enumerate() {
            entries.push(LimitFactorEntry {
                provider: provider.clone(),
                lane: lane.clone(),
                account: multiple_accounts.then(|| format!("account {}", index + 1)),
                usd_per_percent: diagnostic.usd_per_percent,
                method: diagnostic.method,
                sample_count: diagnostic.sample_count,
                point_count: diagnostic.point_count,
                delta_sample_count: diagnostic.delta_sample_count,
                unattributed_sample_count: diagnostic.unattributed_sample_count,
                plan: diagnostic.plan,
                plan_tier: diagnostic.plan_tier,
                residual: diagnostic.residual.map(|residual| LimitResidualEntry {
                    meter_percent: residual.meter_percent,
                    estimated_percent: residual.estimated_percent,
                    computed_at_epoch: residual.computed_at_epoch,
                }),
            });
        }
    }
    Ok(entries)
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
                 );
                 CREATE TABLE provider_usage_period (
                     id INTEGER PRIMARY KEY,
                     provider TEXT NOT NULL,
                     account_key TEXT NOT NULL,
                     window_role TEXT NOT NULL
                 );
                 CREATE TABLE provider_limit_factor_point (
                     provider TEXT NOT NULL,
                     account_key TEXT NOT NULL,
                     lane TEXT NOT NULL,
                     effective_at_epoch INTEGER NOT NULL,
                     usd_per_percent REAL NOT NULL,
                     method TEXT NOT NULL,
                     sample_count INTEGER NOT NULL,
                     plan TEXT,
                     plan_tier TEXT
                 );
                 CREATE TABLE provider_limit_factor_sample (
                     provider TEXT NOT NULL,
                     account_key TEXT NOT NULL,
                     lane TEXT NOT NULL,
                     kind TEXT NOT NULL,
                     to_epoch INTEGER NOT NULL
                 );
                 CREATE TABLE provider_limit_residual (
                     period_id INTEGER PRIMARY KEY,
                     computed_at_epoch INTEGER NOT NULL,
                     meter_percent REAL NOT NULL,
                     estimated_percent REAL NOT NULL
                 );",
            )
            .expect("create the fixture schema");
        connection
    }

    /// A 64-character opaque account key, the shape the export must never
    /// carry. Real keys are hex hashes; the exact characters do not matter
    /// here, only the length the export's own content notice promises to
    /// exclude.
    fn fixture_account_key(character: char) -> String {
        character.to_string().repeat(64)
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
            1_000_000,
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
            1_000_000,
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

    fn insert_factor_point(
        connection: &Connection,
        account_key: &str,
        lane: &str,
        effective_at_epoch: i64,
        usd_per_percent: f64,
        plan: Option<&str>,
    ) {
        connection
            .execute(
                "INSERT INTO provider_limit_factor_point (
                     provider, account_key, lane, effective_at_epoch, usd_per_percent,
                     method, sample_count, plan, plan_tier
                 ) VALUES ('anthropic', ?1, ?2, ?3, ?4, 'delta', 5, ?5, NULL)",
                rusqlite::params![account_key, lane, effective_at_epoch, usd_per_percent, plan],
            )
            .expect("insert a fixture factor point");
    }

    fn insert_factor_sample(
        connection: &Connection,
        account_key: &str,
        lane: &str,
        kind: &str,
        to_epoch: i64,
    ) {
        connection
            .execute(
                "INSERT INTO provider_limit_factor_sample (
                     provider, account_key, lane, kind, to_epoch
                 ) VALUES ('anthropic', ?1, ?2, ?3, ?4)",
                rusqlite::params![account_key, lane, kind, to_epoch],
            )
            .expect("insert a fixture factor sample");
    }

    fn insert_residual(
        connection: &Connection,
        account_key: &str,
        window_role: &str,
        meter_percent: f64,
        estimated_percent: f64,
        computed_at_epoch: i64,
    ) -> i64 {
        connection
            .execute(
                "INSERT INTO provider_usage_period (provider, account_key, window_role)
                 VALUES ('anthropic', ?1, ?2)",
                rusqlite::params![account_key, window_role],
            )
            .expect("insert a fixture period");
        let period_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO provider_limit_residual (
                     period_id, computed_at_epoch, meter_percent, estimated_percent
                 ) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    period_id,
                    computed_at_epoch,
                    meter_percent,
                    estimated_percent
                ],
            )
            .expect("insert a fixture residual");
        period_id
    }

    #[test]
    fn a_database_with_no_learned_factor_exports_an_empty_limit_factors_section() {
        let mut connection = fixture_connection();
        let transaction = connection
            .transaction()
            .expect("start the fixture snapshot");
        let export = build_from_transaction(
            &transaction,
            "0.3.1".to_owned(),
            "2026-09-02T00:00:00Z".to_owned(),
            1_000_000,
        )
        .expect("build the fixture export");
        let value = serde_json::to_value(export).expect("serialize the fixture export");

        assert_eq!(value["limitFactors"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn a_single_account_reports_no_ordinal_and_its_latest_point_and_residual() {
        let mut connection = fixture_connection();
        insert_factor_point(
            &connection,
            &fixture_account_key('a'),
            "fiveHour",
            100,
            0.5,
            Some("max"),
        );
        insert_factor_point(
            &connection,
            &fixture_account_key('a'),
            "fiveHour",
            200,
            0.6,
            Some("max"),
        );
        insert_factor_sample(
            &connection,
            &fixture_account_key('a'),
            "fiveHour",
            "delta",
            900_500,
        );
        // A rollout-history sample is a delta sample from another source
        // and must count alongside it.
        insert_factor_sample(
            &connection,
            &fixture_account_key('a'),
            "fiveHour",
            "rollout",
            900_550,
        );
        insert_factor_sample(
            &connection,
            &fixture_account_key('a'),
            "fiveHour",
            "unattributed",
            900_600,
        );
        insert_residual(
            &connection,
            &fixture_account_key('a'),
            "primaryShort",
            42.0,
            40.0,
            999_000,
        );

        let transaction = connection
            .transaction()
            .expect("start the fixture snapshot");
        let export = build_from_transaction(
            &transaction,
            "0.3.1".to_owned(),
            "2026-09-02T00:00:00Z".to_owned(),
            1_000_000,
        )
        .expect("build the fixture export");
        let json = export.to_json().expect("serialize the fixture export");
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("parse the fixture export");

        let entries = value["limitFactors"].as_array().expect("an array");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry["provider"], "anthropic");
        assert_eq!(entry["lane"], "fiveHour");
        assert!(
            entry.get("account").is_none(),
            "a single account carries no ordinal"
        );
        // The latest point (effective_at_epoch 200), not the earlier one.
        assert_eq!(entry["usdPerPercent"], 0.6);
        assert_eq!(entry["plan"], "max");
        assert_eq!(entry["pointCount"], 2);
        assert_eq!(entry["deltaSampleCount"], 2);
        assert_eq!(entry["unattributedSampleCount"], 1);
        assert_eq!(entry["residual"]["meterPercent"], 42.0);
        assert_eq!(entry["residual"]["estimatedPercent"], 40.0);
        assert_eq!(entry["residual"]["computedAtEpoch"], 999_000);
        assert!(!json.contains(&fixture_account_key('a')));
    }

    #[test]
    fn multiple_accounts_for_one_lane_get_stable_ordinals_and_never_an_account_key() {
        let mut connection = fixture_connection();
        let first = fixture_account_key('a');
        let second = fixture_account_key('b');
        insert_factor_point(&connection, &second, "weekly", 100, 2.0, None);
        insert_factor_point(&connection, &first, "weekly", 100, 1.0, None);

        let transaction = connection
            .transaction()
            .expect("start the fixture snapshot");
        let export = build_from_transaction(
            &transaction,
            "0.3.1".to_owned(),
            "2026-09-02T00:00:00Z".to_owned(),
            1_000_000,
        )
        .expect("build the fixture export");
        let json = export.to_json().expect("serialize the fixture export");
        let value: serde_json::Value =
            serde_json::from_str(&json).expect("parse the fixture export");

        let entries = value["limitFactors"].as_array().expect("an array");
        assert_eq!(entries.len(), 2);
        // Ordered by the (opaque) account key, so the numbering is stable
        // across reads of the same database rather than query-order noise.
        assert_eq!(entries[0]["account"], "account 1");
        assert_eq!(entries[0]["usdPerPercent"], 1.0);
        assert_eq!(entries[1]["account"], "account 2");
        assert_eq!(entries[1]["usdPerPercent"], 2.0);
        assert!(!json.contains(&first));
        assert!(!json.contains(&second));
    }
}
