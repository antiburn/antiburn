//! Read-only search of session metadata in the Antiburn product database.

use std::time::{Duration, Instant};

use rusqlite::{Connection, limits::Limit, params};
use serde::Serialize;

use crate::{
    checks::{SessionIdentity, database::open_read_only, discovery::find_database_path},
    cli::SearchArgs,
    config::Config,
};

/// A local match. The full identity distinguishes sessions that share an ID.
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
pub(crate) async fn lookup(args: SearchArgs, config: Config) -> Result<SearchResponse, String> {
    tokio::task::spawn_blocking(move || {
        let path = find_database_path(&config)?;
        let connection = open_read_only(&path, config.busy_timeout)?;
        let limit = args.limit.unwrap_or(config.search_limit);
        read_sessions(
            &connection,
            &args.query,
            limit,
            config.local_search_timeout,
            config.local_search_sqlite_length_limit_bytes,
        )
        .map_err(|error| format!("could not search Antiburn sessions: {error}"))
    })
    .await
    .map_err(|error| format!("Antiburn session search task failed: {error}"))?
}

/// Read only the session table, which has one row per full session identity.
fn read_sessions(
    connection: &Connection,
    query: &str,
    limit: u16,
    timeout: Duration,
    sqlite_length_limit_bytes: i32,
) -> rusqlite::Result<SearchResponse> {
    let deadline = Instant::now() + timeout;
    connection.progress_handler(1000, Some(move || Instant::now() >= deadline));
    connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, sqlite_length_limit_bytes);
    let mut statement = connection.prepare(
        "WITH searchable AS (
            SELECT environment_key, agent, session_id, updated_at_epoch,
                   CASE WHEN title_source IN ('userRename', 'aiGenerated', 'explicit')
                        THEN title END AS display_title
              FROM session
         )
         SELECT environment_key, agent, session_id, display_title, updated_at_epoch
           FROM searchable
          WHERE instr(lower(session_id), lower(?1)) > 0
             OR instr(lower(display_title), lower(?1)) > 0
          ORDER BY updated_at_epoch DESC, environment_key, agent, session_id
          LIMIT ?2",
    )?;
    let rows = statement.query_map(params![query, i64::from(limit) + 1], |row| {
        Ok(SearchSession {
            identity: SessionIdentity {
                environment_key: row.get(0)?,
                agent: row.get(1)?,
                session_id: row.get(2)?,
            },
            title: row.get(3)?,
            updated_at_epoch: row.get(4)?,
        })
    })?;
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
