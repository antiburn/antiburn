//! Read-only access to session metadata in the Antiburn product database.

use std::time::Instant;

use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Row, limits::Limit, params, params_from_iter,
};
use serde::Serialize;

use crate::{
    checks::{SessionIdentity, database::open_read_only, discovery::find_database_path},
    config::Config,
    document::{IndexedDatabase, LocalSource, SessionDocument, read_local},
};

const SESSION_METADATA: &str = "WITH session_metadata AS (
    SELECT environment_key, agent, session_id, updated_at_epoch,
           CASE WHEN title_source IN ('userRename', 'aiGenerated', 'explicit')
                THEN title END AS display_title
      FROM session
)
SELECT environment_key, agent, session_id, display_title, updated_at_epoch
  FROM session_metadata";

/// Local session metadata. The full identity distinguishes sessions that share an ID.
#[derive(Debug, Serialize)]
pub struct SearchSession {
    pub identity: SessionIdentity,
    /// The stored title, excluding titles marked as excerpts and unknown sources.
    pub title: Option<String>,
    /// Last known activity in Unix seconds, when available.
    pub updated_at_epoch: Option<i64>,
}

/// Source actually queried by this command.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    Local,
}

/// A bounded local result, sorted by most recent activity first.
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub source: SearchSource,
    pub sessions: Vec<SearchSession>,
    /// True if the query found more matches than the result limit.
    pub truncated: bool,
}

/// Open the product database and run one query off the async runtime thread.
pub(crate) async fn lookup(
    query: String,
    limit: Option<u16>,
    config: Config,
) -> Result<SearchResponse, String> {
    tokio::task::spawn_blocking(move || {
        let connection = open_session_database(&config)?;
        let limit = limit.unwrap_or(config.search_limit);
        read_sessions(&connection, &query, limit)
            .map_err(|error| format!("could not search Antiburn sessions: {error}"))
    })
    .await
    .map_err(|error| format!("Antiburn session search task failed: {error}"))?
}

/// Resolve one exact identity and read the indexed source off the runtime thread.
pub(crate) async fn get_local_document(
    session_id: String,
    agent: Option<String>,
    environment: Option<String>,
    config: Config,
) -> Result<SessionDocument, String> {
    tokio::task::spawn_blocking(move || {
        let mut connection = open_session_database(&config)?;
        let snapshot = connection.transaction().map_err(|error| local_index_error(&error))?;
        let mut matches = read_exact_matches(
            &snapshot,
            &session_id,
            agent.as_deref(),
            environment.as_deref(),
        )
        .map_err(|error| local_index_error(&error))?;
        if matches.len() > 1 {
            return Err(format!(
                "session ID {session_id:?} matches multiple local Antiburn sessions; use --agent and --environment to select one"
            ));
        }
        let mut source = matches.pop().ok_or_else(|| {
            format!("session ID {session_id:?} was not found in the local Antiburn database")
        })?;
        if source.is_opencode_v2() {
            source.database = read_indexed_database(&snapshot, &source.identity)
                .map_err(|error| local_index_error(&error))?;
        }
        drop(snapshot);
        drop(connection);
        read_local(source)
    })
    .await
    .map_err(|error| format!("local Antiburn session lookup task failed: {error}"))?
}

fn local_index_error(error: &rusqlite::Error) -> &'static str {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => {
            "the local session index is busy; try again"
        }
        Some(ErrorCode::OperationInterrupted) => {
            "the local session index read exceeded the time limit; increase local_search_timeout_ms"
        }
        Some(ErrorCode::TooBig) => {
            "the local session index read exceeded the size limit; raise local_search_sqlite_length_limit_bytes if it is below the maximum"
        }
        _ => "could not read the local session source index",
    }
}

/// Read matches and optional accepted source keys from the same SQLite snapshot.
#[cfg(feature = "cloud")]
pub(crate) async fn lookup_with_receipts(
    query: String,
    limit: u16,
    config: Config,
    workspace_id: String,
    origin: String,
) -> Result<(SearchResponse, Vec<Option<String>>), String> {
    tokio::task::spawn_blocking(move || {
        let mut connection = open_session_database(&config)?;
        let read = || "could not search local Antiburn sessions and source receipts";
        let snapshot = connection.transaction().map_err(|_| read())?;
        let result = read_sessions(&snapshot, &query, limit).map_err(|_| read())?;
        let lookup = crate::cloud_identity::ReceiptLookup::new(&snapshot, &workspace_id, &origin)
            .map_err(|_| read())?;
        let keys = result
            .sessions
            .iter()
            .map(|row| match &lookup {
                Some(lookup) => lookup.source_key(&row.identity),
                None => Ok(None),
            })
            .collect::<rusqlite::Result<_>>()
            .map_err(|_| read())?;
        Ok((result, keys))
    })
    .await
    .map_err(|_| "local Antiburn session search task failed")?
}

/// Open the product store with the shared local metadata read limits.
fn open_session_database(config: &Config) -> Result<Connection, String> {
    let path = find_database_path(config)?;
    let connection = open_read_only(&path, config.busy_timeout)?;
    let deadline = Instant::now() + config.local_search_timeout;
    connection.progress_handler(1000, Some(move || Instant::now() >= deadline));
    connection.set_limit(
        Limit::SQLITE_LIMIT_LENGTH,
        config.local_search_sqlite_length_limit_bytes,
    );
    Ok(connection)
}

/// Read at most two matches. A second row is enough to reject an ambiguous ID.
fn read_exact_matches(
    connection: &Connection,
    session_id: &str,
    agent: Option<&str>,
    environment: Option<&str>,
) -> rusqlite::Result<Vec<LocalSource>> {
    // Keep equality terms visible so SQLite can use the composite primary key.
    let agent_filter = if agent.is_some() {
        " AND agent = ?"
    } else {
        ""
    };
    let environment_filter = if environment.is_some() {
        " AND environment_key = ?"
    } else {
        ""
    };
    let mut statement = connection.prepare(&format!(
        "SELECT environment_key, agent, session_id, source_kind, source_label, source_format,
               CASE WHEN title_source IN ('userRename', 'aiGenerated', 'explicit')
                    THEN title END
           FROM session
         WHERE session_id = ?{agent_filter}{environment_filter}
         LIMIT 2"
    ))?;
    let selectors = [Some(session_id), agent, environment].into_iter().flatten();
    let rows = statement.query_map(params_from_iter(selectors), |row| {
        Ok(LocalSource {
            identity: read_identity(row)?,
            kind: row.get(3)?,
            label: row.get(4)?,
            format: row.get(5)?,
            title: row.get(6)?,
            database: None,
        })
    })?;
    rows.collect()
}

/// Read the provider database binding from the same snapshot as the exact index row.
fn read_indexed_database(
    connection: &Connection,
    identity: &SessionIdentity,
) -> rusqlite::Result<Option<IndexedDatabase>> {
    connection
        .query_row(
            "SELECT fingerprint_source_path, source_fingerprint FROM session
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
               AND fingerprint_source_path IS NOT NULL AND source_fingerprint IS NOT NULL",
            params![
                identity.environment_key,
                identity.agent,
                identity.session_id
            ],
            |row| {
                Ok(IndexedDatabase {
                    path: row.get(0)?,
                    fingerprint: row.get(1)?,
                })
            },
        )
        .optional()
}

/// Read only the session table, which has one row per full session identity.
fn read_sessions(
    connection: &Connection,
    query: &str,
    limit: u16,
) -> rusqlite::Result<SearchResponse> {
    let mut statement = connection.prepare(&format!(
        "{SESSION_METADATA}
         WHERE instr(lower(session_id), lower(?1)) > 0
            OR instr(lower(display_title), lower(?1)) > 0
         ORDER BY updated_at_epoch DESC, environment_key, agent, session_id
         LIMIT ?2"
    ))?;
    let rows = statement.query_map(params![query, i64::from(limit) + 1], read_metadata)?;
    let mut sessions = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let limit = usize::from(limit);
    let truncated = sessions.len() > limit;
    sessions.truncate(limit);
    Ok(SearchResponse {
        source: SearchSource::Local,
        sessions,
        truncated,
    })
}

fn read_metadata(row: &Row<'_>) -> rusqlite::Result<SearchSession> {
    Ok(SearchSession {
        identity: read_identity(row)?,
        title: row.get(3)?,
        updated_at_epoch: row.get(4)?,
    })
}

fn read_identity(row: &Row<'_>) -> rusqlite::Result<SessionIdentity> {
    Ok(SessionIdentity {
        environment_key: row.get(0)?,
        agent: row.get(1)?,
        session_id: row.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_the_local_index_deadline() {
        let interrupted = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
            None,
        );
        assert!(local_index_error(&interrupted).contains("exceeded the time limit"));
        assert!(local_index_error(&interrupted).contains("local_search_timeout_ms"));
    }
}
