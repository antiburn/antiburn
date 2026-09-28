//! Full-text search over retained session metadata.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use antiburn_local::repositories::repo_root_identity_for_platform;
use anyhow::{Result, ensure};
use base64::Engine as _;
use rusqlite::functions::FunctionFlags;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::Store;
use crate::session_search_scope::SessionSearchScope;

pub const SESSION_SEARCH_PAGE_SIZE: usize = 20;
const SESSION_SEARCH_BACKFILL_BATCH_SIZE: usize = 64;
const SESSION_SEARCH_QUERY_MAX_CHARS: usize = 200;
const SESSION_SEARCH_CURSOR_MAX_BYTES: usize = 2_048;
const SESSION_SEARCH_CURSOR_VERSION: u8 = 1;
const SESSION_SEARCH_PROGRESS_INTERVAL: i32 = 1_000;
const SESSION_SEARCH_VM_STEP_LIMIT: u64 = 50_000_000;
const SESSION_SEARCH_BUDGET_ERROR: &str =
    "session search exceeded its local work limit; refine the search and try again";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSearchResult {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
    pub wsl_distro: Option<String>,
    pub title: Option<String>,
    pub repository: String,
    pub cwd_label: String,
    pub models: Vec<String>,
    pub updated_at_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSearchPage {
    pub results: Vec<SessionSearchResult>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub indexing: bool,
}

#[derive(Debug)]
struct RankedSearchResult {
    result: SessionSearchResult,
    exact_rank: i64,
    updated_at_epoch: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionSearchCursor {
    version: u8,
    query_digest: String,
    generation: i64,
    exact_rank: i64,
    updated_at_epoch: i64,
    environment_key: String,
    agent: String,
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexedModelRun {
    model: String,
}

struct SearchProgressGuard<'a> {
    connection: &'a rusqlite::Connection,
}

impl<'a> SearchProgressGuard<'a> {
    fn install(connection: &'a rusqlite::Connection, vm_step_limit: u64) -> Self {
        let callback_limit = vm_step_limit
            .div_ceil(SESSION_SEARCH_PROGRESS_INTERVAL as u64)
            .max(1);
        let callbacks = Arc::new(AtomicU64::new(0));
        connection.progress_handler(
            SESSION_SEARCH_PROGRESS_INTERVAL,
            Some(move || callbacks.fetch_add(1, Ordering::Relaxed) + 1 >= callback_limit),
        );
        Self { connection }
    }
}

impl Drop for SearchProgressGuard<'_> {
    fn drop(&mut self) {
        self.connection.progress_handler(0, None::<fn() -> bool>);
    }
}

fn session_search_query_error(error: rusqlite::Error) -> anyhow::Error {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.code == rusqlite::ErrorCode::OperationInterrupted
    ) {
        anyhow::anyhow!(SESSION_SEARCH_BUDGET_ERROR)
    } else {
        error.into()
    }
}

pub(super) fn register_session_search_functions(connection: &rusqlite::Connection) -> Result<()> {
    connection.create_scalar_function(
        "session_search_path_identity",
        2,
        FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_UTF8,
        |context| {
            let path = context.get::<String>(0)?;
            let wsl_distro = context.get::<Option<String>>(1)?;
            let bytes = path.as_bytes();
            let is_windows_path = wsl_distro.is_none()
                && (path.starts_with(r"\\")
                    || path.starts_with("//")
                    || bytes.get(1) == Some(&b':'));
            Ok(repo_root_identity_for_platform(
                Path::new(&path),
                is_windows_path,
            ))
        },
    )?;
    Ok(())
}

impl Store {
    #[cfg(test)]
    pub(crate) fn session_search_generation(&self) -> Result<i64> {
        session_search_generation_in(&self.lock())
    }

    #[cfg(test)]
    pub(crate) fn local_session_candidates(
        &self,
        query: &str,
        scope: Option<&SessionSearchScope>,
    ) -> Result<(Vec<SessionSearchResult>, i64)> {
        self.local_session_candidates_with_vm_step_limit(query, SESSION_SEARCH_VM_STEP_LIMIT, scope)
    }

    /// Reads the existing metadata index without advancing its backfill.
    pub(crate) fn readonly_local_session_candidates(
        &self,
        query: &str,
        scope: Option<&SessionSearchScope>,
    ) -> Result<(Vec<SessionSearchResult>, i64)> {
        self.readonly_local_session_candidates_with_vm_step_limit(
            query,
            SESSION_SEARCH_VM_STEP_LIMIT,
            scope,
        )
    }

    fn readonly_local_session_candidates_with_vm_step_limit(
        &self,
        query: &str,
        vm_step_limit: u64,
        scope: Option<&SessionSearchScope>,
    ) -> Result<(Vec<SessionSearchResult>, i64)> {
        ensure!(
            query.chars().count() <= SESSION_SEARCH_QUERY_MAX_CHARS,
            "query too long"
        );
        let expression = relaxed_fts_expression(query);
        let connection = self.lock();
        let generation = session_search_generation_in(&connection)?;
        local_session_candidates_in(
            &connection,
            query,
            &expression,
            generation,
            vm_step_limit,
            scope,
        )
    }

    #[cfg(test)]
    fn local_session_candidates_with_vm_step_limit(
        &self,
        query: &str,
        vm_step_limit: u64,
        scope: Option<&SessionSearchScope>,
    ) -> Result<(Vec<SessionSearchResult>, i64)> {
        ensure!(
            query.chars().count() <= SESSION_SEARCH_QUERY_MAX_CHARS,
            "query too long"
        );
        let expression = relaxed_fts_expression(query);
        let mut connection = self.lock();
        advance_session_search_backfill_in(&mut connection)?;
        let generation = session_search_generation_in(&connection)?;
        local_session_candidates_in(
            &connection,
            query,
            &expression,
            generation,
            vm_step_limit,
            scope,
        )
    }

    /// Search every indexed session without loading transcript content.
    pub(crate) fn search_sessions(
        &self,
        query: &str,
        cursor: Option<&str>,
        scope: Option<&SessionSearchScope>,
    ) -> Result<SessionSearchPage> {
        self.search_sessions_with_vm_step_limit(query, cursor, SESSION_SEARCH_VM_STEP_LIMIT, scope)
    }

    fn search_sessions_with_vm_step_limit(
        &self,
        query: &str,
        cursor: Option<&str>,
        vm_step_limit: u64,
        scope: Option<&SessionSearchScope>,
    ) -> Result<SessionSearchPage> {
        let expression = literal_fts_expression(query)?;
        let exact_query = query.trim();
        let digest = query_digest(&format!(
            "{}:{}",
            exact_query,
            serde_json::to_string(&scope)?
        ));
        let cursor = decode_cursor(cursor, &digest)?;
        let mut connection = self.lock();
        let indexing = if cursor.is_none() {
            advance_session_search_backfill_in(&mut connection)?
        } else {
            session_search_indexing_in(&connection)?
        };
        let generation = session_search_generation_in(&connection)?;
        if let Some(cursor) = &cursor {
            ensure!(
                cursor.generation == generation,
                "stale session search cursor"
            );
        }
        if expression.is_empty() {
            return Ok(SessionSearchPage {
                results: Vec::new(),
                next_cursor: None,
                has_more: false,
                indexing,
            });
        }

        let progress_guard = SearchProgressGuard::install(&connection, vm_step_limit);
        let ranked_result = (|| -> rusqlite::Result<Vec<RankedSearchResult>> {
            let mut statement = connection.prepare(
                "WITH ranked_keys AS MATERIALIZED (
                 SELECT *
                   FROM (
                       SELECT document.environment_key,
                              document.agent,
                              document.session_id,
                              document.wsl_distro,
                              NULLIF(document.title, '') AS title,
                              document.repository,
                              document.cwd,
                              document.updated_at_epoch,
                              CASE
                                  WHEN lower(document.session_id) = lower(?2) THEN 0
                                  WHEN lower(document.title) = lower(?2) THEN 1
                                  ELSE 2
                              END AS exact_rank
                         FROM session_search_fts
                         JOIN session_search_document AS document
                           ON document.id = session_search_fts.rowid
                        WHERE session_search_fts MATCH ?1
                          AND (?9 IS NULL OR document.updated_at_epoch BETWEEN ?9 AND ?10)
                   ) AS matched
                  WHERE ?4 IS NULL
                     OR exact_rank > ?4
                     OR (exact_rank = ?4 AND updated_at_epoch < ?5)
                     OR (exact_rank = ?4 AND updated_at_epoch = ?5
                         AND environment_key > ?6)
                     OR (exact_rank = ?4 AND updated_at_epoch = ?5
                         AND environment_key = ?6 AND agent > ?7)
                     OR (exact_rank = ?4 AND updated_at_epoch = ?5
                         AND environment_key = ?6 AND agent = ?7 AND session_id > ?8)
                  ORDER BY exact_rank,
                           updated_at_epoch DESC,
                           environment_key,
                           agent,
                           session_id
                  LIMIT ?3
             )
             SELECT ranked_keys.environment_key,
                    ranked_keys.agent,
                    ranked_keys.session_id,
                    ranked_keys.wsl_distro,
                    ranked_keys.title,
                    ranked_keys.repository,
                    ranked_keys.cwd,
                    analysis.inclusive_models_json,
                    ranked_keys.updated_at_epoch,
                    ranked_keys.exact_rank
               FROM ranked_keys
               LEFT JOIN session_analysis AS analysis
                 ON analysis.environment_key = ranked_keys.environment_key
                AND analysis.agent = ranked_keys.agent
                AND analysis.session_id = ranked_keys.session_id
              ORDER BY ranked_keys.exact_rank,
                       ranked_keys.updated_at_epoch DESC,
                       ranked_keys.environment_key,
                       ranked_keys.agent,
                       ranked_keys.session_id",
            )?;
            let fetch_count = i64::try_from(SESSION_SEARCH_PAGE_SIZE + 1)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            let rows = statement.query_map(
                params![
                    expression,
                    exact_query,
                    fetch_count,
                    cursor.as_ref().map(|value| value.exact_rank),
                    cursor.as_ref().map(|value| value.updated_at_epoch),
                    cursor.as_ref().map(|value| value.environment_key.as_str()),
                    cursor.as_ref().map(|value| value.agent.as_str()),
                    cursor.as_ref().map(|value| value.session_id.as_str()),
                    scope.map(|scope| scope.from_epoch),
                    scope.map(|scope| scope.through_epoch),
                ],
                |row| {
                    let repository = row.get::<_, String>(5)?;
                    let cwd = row.get::<_, String>(6)?;
                    let models_json = row
                        .get::<_, Option<String>>(7)?
                        .unwrap_or_else(|| "[]".to_owned());
                    let updated_at_epoch = row.get::<_, i64>(8)?;
                    Ok(RankedSearchResult {
                        result: SessionSearchResult {
                            environment_key: row.get(0)?,
                            agent: row.get(1)?,
                            session_id: row.get(2)?,
                            wsl_distro: row.get(3)?,
                            title: row.get(4)?,
                            repository: repository_label(&repository, &cwd),
                            cwd_label: path_label(&cwd),
                            models: model_names(&models_json),
                            updated_at_epoch: (updated_at_epoch != 0).then_some(updated_at_epoch),
                        },
                        exact_rank: row.get(9)?,
                        updated_at_epoch,
                    })
                },
            )?;
            rows.collect()
        })();
        drop(progress_guard);
        let mut ranked = ranked_result.map_err(session_search_query_error)?;
        let has_more = ranked.len() > SESSION_SEARCH_PAGE_SIZE;
        ranked.truncate(SESSION_SEARCH_PAGE_SIZE);
        let next_cursor = if has_more {
            ranked
                .last()
                .map(|last| encode_cursor(last, &digest, generation))
                .transpose()?
        } else {
            None
        };
        Ok(SessionSearchPage {
            results: ranked.into_iter().map(|ranked| ranked.result).collect(),
            next_cursor,
            has_more,
            indexing,
        })
    }

    /// Refresh repository names after the repository inventory changes.
    pub(super) fn refresh_session_search_repositories_in(
        connection: &rusqlite::Connection,
    ) -> Result<()> {
        connection.execute(
            "WITH labels AS (
                 SELECT document.id,
                        COALESCE((
                            SELECT repository.repo_name
                              FROM repository
                             WHERE repository.repo_root IS NOT NULL
                               AND COALESCE(repository.wsl_distro, '') =
                                   COALESCE(document.wsl_distro, '')
                               AND (
                                   session_search_path_identity(
                                       document.cwd, document.wsl_distro
                                   ) = session_search_path_identity(
                                       repository.repo_root, repository.wsl_distro
                                   )
                                   OR substr(
                                       session_search_path_identity(
                                           document.cwd, document.wsl_distro
                                       ),
                                       1,
                                       length(session_search_path_identity(
                                           repository.repo_root, repository.wsl_distro
                                       )) + 1
                                   ) = session_search_path_identity(
                                       repository.repo_root, repository.wsl_distro
                                   ) || '/'
                               )
                             ORDER BY length(repository.repo_root) DESC, repository.key
                             LIMIT 1
                        ), '') AS label
                   FROM session_search_document AS document
             )
             UPDATE session_search_document AS document
                SET repository = (
                    SELECT label FROM labels WHERE labels.id = document.id
                )
              WHERE repository <> (
                    SELECT label FROM labels WHERE labels.id = document.id
                )",
            [],
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn rebuild_session_search_index(&self) -> Result<()> {
        self.lock().execute(
            "INSERT INTO session_search_fts(session_search_fts) VALUES ('rebuild')",
            [],
        )?;
        Ok(())
    }
}

fn local_session_candidates_in(
    connection: &rusqlite::Connection,
    query: &str,
    expression: &str,
    generation: i64,
    vm_step_limit: u64,
    scope: Option<&SessionSearchScope>,
) -> Result<(Vec<SessionSearchResult>, i64)> {
    if expression.is_empty() {
        return Ok((Vec::new(), generation));
    }
    let _budget = SearchProgressGuard::install(connection, vm_step_limit);
    let mut statement = connection.prepare(
        "WITH ranked_keys AS MATERIALIZED (
             SELECT d.environment_key, d.agent, d.session_id, d.wsl_distro, d.title,
                    d.repository, d.cwd, d.updated_at_epoch,
                    (lower(d.session_id) = lower(?2)) AS exact_id,
                    (lower(d.title) = lower(?2)) AS exact_title,
                    bm25(session_search_fts) AS relevance
               FROM session_search_fts
               JOIN session_search_document d ON d.rowid = session_search_fts.rowid
              WHERE session_search_fts MATCH ?1
                AND (?3 IS NULL OR d.updated_at_epoch BETWEEN ?3 AND ?4)
              ORDER BY exact_id DESC, exact_title DESC, relevance,
                       d.updated_at_epoch DESC, d.environment_key, d.agent, d.session_id
              LIMIT 8
         )
         SELECT d.environment_key, d.agent, d.session_id, d.wsl_distro, d.title,
                d.repository, d.cwd, a.inclusive_models_json, d.updated_at_epoch
           FROM ranked_keys d
           LEFT JOIN session_analysis a ON a.environment_key = d.environment_key
                AND a.agent = d.agent AND a.session_id = d.session_id
          ORDER BY d.exact_id DESC, d.exact_title DESC, d.relevance,
                   d.updated_at_epoch DESC, d.environment_key, d.agent, d.session_id",
    )?;
    let rows = statement.query_map(
        params![
            expression,
            query.trim(),
            scope.map(|s| s.from_epoch),
            scope.map(|s| s.through_epoch)
        ],
        |row| {
            let repository: String = row.get(5)?;
            let cwd: String = row.get(6)?;
            let models: Option<String> = row.get(7)?;
            let timestamp: i64 = row.get(8)?;
            Ok(SessionSearchResult {
                environment_key: row.get(0)?,
                agent: row.get(1)?,
                session_id: row.get(2)?,
                wsl_distro: row.get(3)?,
                title: row.get(4)?,
                repository: repository_label(&repository, &cwd),
                cwd_label: path_label(&cwd),
                models: model_names(models.as_deref().unwrap_or("[]")),
                updated_at_epoch: (timestamp != 0).then_some(timestamp),
            })
        },
    )?;
    let results = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(session_search_query_error)?;
    Ok((results, generation))
}

fn advance_session_search_backfill_in(connection: &mut rusqlite::Connection) -> Result<bool> {
    let transaction = connection.transaction()?;
    let (backfill_rowid, backfill_target_rowid, backfill_complete) = transaction.query_row(
        "SELECT backfill_rowid, backfill_target_rowid, backfill_complete
           FROM session_search_state
          WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, bool>(2)?,
            ))
        },
    )?;
    if backfill_complete {
        transaction.commit()?;
        return Ok(false);
    }

    let rowids = {
        let mut statement = transaction.prepare(
            "SELECT rowid
              FROM session
              WHERE rowid > ?1
                AND rowid <= ?2
              ORDER BY rowid
              LIMIT ?3",
        )?;
        statement
            .query_map(
                params![
                    backfill_rowid,
                    backfill_target_rowid,
                    i64::try_from(SESSION_SEARCH_BACKFILL_BATCH_SIZE + 1)?
                ],
                |row| row.get::<_, i64>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let Some(batch_end_rowid) = rowids
        .get(SESSION_SEARCH_BACKFILL_BATCH_SIZE.saturating_sub(1))
        .or_else(|| rowids.last())
        .copied()
    else {
        transaction.execute(
            "UPDATE session_search_state
                SET backfill_complete = 1
              WHERE id = 1",
            [],
        )?;
        transaction.commit()?;
        return Ok(false);
    };
    let indexing = rowids.len() > SESSION_SEARCH_BACKFILL_BATCH_SIZE;

    transaction.execute(
        "INSERT INTO session_search_document(
             environment_key, agent, agent_aliases, session_id, wsl_distro,
             title, repository, cwd, models, updated_at_epoch
         )
         SELECT session.environment_key,
                session.agent,
                CASE session.agent
                    WHEN 'claude-code' THEN 'Claude Code Claude'
                    WHEN 'codex' THEN 'Codex OpenAI'
                    WHEN 'copilot' THEN 'GitHub Copilot'
                    WHEN 'opencode' THEN 'OpenCode'
                    WHEN 'amp-code' THEN 'Amp'
                    ELSE replace(session.agent, '-', ' ')
                END,
                session.session_id,
                session.wsl_distro,
                COALESCE(session.title, ''),
                COALESCE((
                    SELECT repository.repo_name
                      FROM repository
                     WHERE repository.repo_root IS NOT NULL
                       AND COALESCE(repository.wsl_distro, '') =
                           COALESCE(session.wsl_distro, '')
                       AND (
                           session_search_path_identity(
                               COALESCE(session.cwd, ''), session.wsl_distro
                           ) = session_search_path_identity(
                               repository.repo_root, repository.wsl_distro
                           )
                           OR substr(
                               session_search_path_identity(
                                   COALESCE(session.cwd, ''), session.wsl_distro
                               ),
                               1,
                               length(session_search_path_identity(
                                   repository.repo_root, repository.wsl_distro
                               )) + 1
                           ) = session_search_path_identity(
                               repository.repo_root, repository.wsl_distro
                           ) || '/'
                       )
                     ORDER BY length(repository.repo_root) DESC, repository.key
                     LIMIT 1
                ), ''),
                COALESCE(session.cwd, ''),
                COALESCE((
                    SELECT GROUP_CONCAT(json_extract(value, '$.model'), ' ')
                      FROM json_each(COALESCE(session_analysis.inclusive_models_json, '[]'))
                     WHERE json_extract(value, '$.model') IS NOT NULL
                ), ''),
                COALESCE(session.updated_at_epoch, 0)
           FROM session
           LEFT JOIN session_analysis
             ON session_analysis.environment_key = session.environment_key
            AND session_analysis.agent = session.agent
            AND session_analysis.session_id = session.session_id
          WHERE session.rowid > ?1
            AND session.rowid <= ?2
          ORDER BY session.rowid
         ON CONFLICT(environment_key, agent, session_id) DO NOTHING",
        params![backfill_rowid, batch_end_rowid],
    )?;
    transaction.execute(
        "UPDATE session_search_state
            SET backfill_rowid = ?1,
                backfill_complete = ?2
          WHERE id = 1",
        params![batch_end_rowid, !indexing],
    )?;
    transaction.commit()?;
    Ok(indexing)
}

fn session_search_indexing_in(connection: &rusqlite::Connection) -> Result<bool> {
    Ok(connection.query_row(
        "SELECT backfill_complete = 0
           FROM session_search_state
          WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

fn session_search_generation_in(connection: &rusqlite::Connection) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT generation FROM session_search_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

fn literal_fts_expression(query: &str) -> Result<String> {
    ensure!(
        query.chars().count() <= SESSION_SEARCH_QUERY_MAX_CHARS,
        "session search query must not exceed {SESSION_SEARCH_QUERY_MAX_CHARS} characters"
    );
    Ok(query
        .split_whitespace()
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND "))
}

fn relaxed_fts_expression(query: &str) -> String {
    const FILLER: &[&str] = &[
        "show",
        "find",
        "me",
        "my",
        "the",
        "a",
        "an",
        "about",
        "where",
        "did",
        "i",
        "please",
        "session",
        "sessions",
        "conversation",
        "conversations",
        "on",
        "in",
        "with",
        "from",
        "that",
        "which",
        "for",
        "to",
        "of",
        "and",
    ];
    query
        .split_whitespace()
        .filter(|term| !FILLER.contains(&term.to_lowercase().as_str()))
        .take(12)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn query_digest(query: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(query.as_bytes()))
}

fn decode_cursor(cursor: Option<&str>, digest: &str) -> Result<Option<SessionSearchCursor>> {
    let Some(encoded) = cursor else {
        return Ok(None);
    };
    ensure!(
        !encoded.is_empty() && encoded.len() <= SESSION_SEARCH_CURSOR_MAX_BYTES,
        "invalid session search cursor"
    );
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("invalid session search cursor"))?;
    let decoded = serde_json::from_slice::<SessionSearchCursor>(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid session search cursor"))?;
    ensure!(
        decoded.version == SESSION_SEARCH_CURSOR_VERSION
            && decoded.query_digest == digest
            && (0..=2).contains(&decoded.exact_rank)
            && !decoded.environment_key.is_empty()
            && !decoded.agent.is_empty()
            && !decoded.session_id.is_empty(),
        "invalid session search cursor"
    );
    Ok(Some(decoded))
}

fn encode_cursor(last: &RankedSearchResult, digest: &str, generation: i64) -> Result<String> {
    let cursor = SessionSearchCursor {
        version: SESSION_SEARCH_CURSOR_VERSION,
        query_digest: digest.to_owned(),
        generation,
        exact_rank: last.exact_rank,
        updated_at_epoch: last.updated_at_epoch,
        environment_key: last.result.environment_key.clone(),
        agent: last.result.agent.clone(),
        session_id: last.result.session_id.clone(),
    };
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor)?))
}

pub(super) fn repository_label(repository: &str, cwd: &str) -> String {
    if repository.is_empty() {
        path_label(cwd)
    } else {
        repository.to_owned()
    }
}

pub(super) fn path_label(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

pub(super) fn model_names(models_json: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    serde_json::from_str::<Vec<IndexedModelRun>>(models_json)
        .unwrap_or_default()
        .into_iter()
        .map(|run| run.model.trim().to_owned())
        .filter(|model| !model.is_empty() && seen.insert(model.clone()))
        .collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod local_candidates_tests;
