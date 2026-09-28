//! Read-only, resumable access to all published retained session content.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::{OptionalExtension as _, params};

use super::Store;
use super::session_search::{SessionSearchResult, model_names, path_label, repository_label};

const READ_CHUNK_BYTES: usize = 256 * 1024;
const SQLITE_PROGRESS_INTERVAL: i32 = 1_000;
const DEEP_MANIFEST_MAX_SESSIONS: usize = 50_000;
const DEEP_MANIFEST_BYTE_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct DeepSessionIdentity {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
}

#[derive(Clone, Debug)]
pub(crate) struct DeepSessionManifest {
    pub identity: DeepSessionIdentity,
    pub session: SessionSearchResult,
    pub source_generation: i64,
    pub published_fence: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeepContentCursor {
    pub thread_id: String,
    pub turn_index: i64,
    pub turn_rowid: i64,
    pub part_index: i64,
    pub byte_offset: usize,
    pub complete: bool,
}

impl Default for DeepContentCursor {
    fn default() -> Self {
        Self {
            thread_id: String::new(),
            turn_index: -1,
            turn_rowid: -1,
            part_index: -1,
            byte_offset: 0,
            complete: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DeepContentChunk {
    pub turn_rowid: i64,
    pub source_key: String,
    pub thread_id: String,
    pub turn_index: i64,
    pub scope: String,
    pub part_index: i64,
    pub kind: String,
    pub content: Vec<u8>,
    pub byte_offset: usize,
    pub original_bytes: usize,
    pub stored_truncated: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DeepContentQuantum {
    pub chunks: Vec<DeepContentChunk>,
    pub inspected_parts: usize,
    pub inspected_bytes: usize,
    pub ingestion_truncated_parts: usize,
    pub complete: bool,
    pub unavailable: bool,
    pub changed: bool,
}

struct CancellationGuard<'a> {
    connection: &'a rusqlite::Connection,
}

impl<'a> CancellationGuard<'a> {
    fn install(connection: &'a rusqlite::Connection, cancelled: Arc<AtomicBool>) -> Self {
        connection.progress_handler(
            SQLITE_PROGRESS_INTERVAL,
            Some(move || cancelled.load(Ordering::Acquire)),
        );
        Self { connection }
    }
}

impl Drop for CancellationGuard<'_> {
    fn drop(&mut self) {
        self.connection.progress_handler(0, None::<fn() -> bool>);
    }
}

impl Store {
    /// Lists each published session once without depending on the metadata index.
    pub(crate) fn deep_session_manifest(
        &self,
        preferred: &[DeepSessionIdentity],
        scope: Option<&crate::session_search_scope::SessionSearchScope>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Vec<DeepSessionManifest>> {
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let guard = CancellationGuard::install(&transaction, Arc::clone(&cancelled));
        anyhow::ensure!(!cancelled.load(Ordering::Acquire), "search cancelled");
        let mut statement = transaction.prepare(
            "SELECT session.environment_key, session.agent, session.session_id,
                    session.wsl_distro, NULLIF(session.title, ''),
                    COALESCE(session.cwd, ''), analysis.inclusive_models_json,
                    session.updated_at_epoch, session.source_generation,
                    evidence.published_fence
               FROM session
               JOIN session_evidence AS evidence
                 ON evidence.environment_key = session.environment_key
                AND evidence.agent = session.agent
                AND evidence.session_id = session.session_id
               LEFT JOIN session_analysis AS analysis
                 ON analysis.environment_key = session.environment_key
                AND analysis.agent = session.agent
                AND analysis.session_id = session.session_id
              WHERE evidence.published_fence IS NOT NULL
                AND evidence.analyzed_generation = session.source_generation
                AND (?1 IS NULL OR session.updated_at_epoch BETWEEN ?1 AND ?2)
              ORDER BY COALESCE(session.updated_at_epoch, 0) DESC,
                       session.session_id DESC, session.environment_key DESC,
                       session.agent DESC",
        )?;
        let rows = statement.query_map(
            rusqlite::params![scope.map(|s| s.from_epoch), scope.map(|s| s.through_epoch)],
            |row| {
                let environment_key = row.get::<_, String>(0)?;
                let agent = row.get::<_, String>(1)?;
                let session_id = row.get::<_, String>(2)?;
                let cwd = row.get::<_, String>(5)?;
                let models = row
                    .get::<_, Option<String>>(6)?
                    .unwrap_or_else(|| "[]".to_owned());
                let identity = DeepSessionIdentity {
                    environment_key: environment_key.clone(),
                    agent: agent.clone(),
                    session_id: session_id.clone(),
                };
                Ok(DeepSessionManifest {
                    identity,
                    session: SessionSearchResult {
                        environment_key,
                        agent,
                        session_id,
                        wsl_distro: row.get(3)?,
                        title: row.get(4)?,
                        repository: repository_label("", &cwd),
                        cwd_label: path_label(&cwd),
                        models: model_names(&models),
                        updated_at_epoch: row.get(7)?,
                    },
                    source_generation: row.get(8)?,
                    published_fence: row.get(9)?,
                })
            },
        )?;
        let mut manifests = Vec::new();
        let mut manifest_bytes = 0;
        for row in rows {
            anyhow::ensure!(!cancelled.load(Ordering::Acquire), "search cancelled");
            let manifest = row?;
            anyhow::ensure!(
                push_manifest_within_budget(
                    &mut manifests,
                    &mut manifest_bytes,
                    manifest,
                    DEEP_MANIFEST_MAX_SESSIONS,
                    DEEP_MANIFEST_BYTE_LIMIT,
                ),
                "resource_limit"
            );
        }
        drop(statement);
        anyhow::ensure!(!cancelled.load(Ordering::Acquire), "search cancelled");
        drop(guard);
        transaction.commit()?;

        if !preferred.is_empty() {
            let priorities = preferred
                .iter()
                .enumerate()
                .map(|(index, key)| (key, index))
                .collect::<HashMap<_, _>>();
            manifests.sort_by_key(|manifest| {
                priorities
                    .get(&manifest.identity)
                    .copied()
                    .map_or((1, usize::MAX), |index| (0, index))
            });
        }
        Ok(manifests)
    }

    /// Reads one bounded session quantum through a short read transaction.
    pub(crate) fn read_deep_content_quantum(
        &self,
        manifest: &DeepSessionManifest,
        cursor: &mut DeepContentCursor,
        byte_limit: usize,
        scheduling_limit: Duration,
        cancelled: Arc<AtomicBool>,
    ) -> Result<DeepContentQuantum> {
        if cursor.complete || byte_limit == 0 {
            return Ok(DeepContentQuantum {
                chunks: Vec::new(),
                inspected_parts: 0,
                inspected_bytes: 0,
                ingestion_truncated_parts: 0,
                complete: cursor.complete,
                unavailable: false,
                changed: false,
            });
        }
        let mut next_cursor = cursor.clone();
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let guard = CancellationGuard::install(&transaction, Arc::clone(&cancelled));
        let current_version = transaction
            .query_row(
                "SELECT session.source_generation, evidence.published_fence
                   FROM session
                   JOIN session_evidence AS evidence
                     ON evidence.environment_key = session.environment_key
                    AND evidence.agent = session.agent
                    AND evidence.session_id = session.session_id
                  WHERE session.environment_key = ?1 AND session.agent = ?2
                    AND session.session_id = ?3
                    AND evidence.published_fence IS NOT NULL",
                params![
                    manifest.identity.environment_key,
                    manifest.identity.agent,
                    manifest.identity.session_id,
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if current_version != Some((manifest.source_generation, manifest.published_fence)) {
            let changed = current_version.is_some();
            drop(guard);
            transaction.commit()?;
            return Ok(DeepContentQuantum {
                chunks: Vec::new(),
                inspected_parts: 0,
                inspected_bytes: 0,
                ingestion_truncated_parts: 0,
                complete: true,
                unavailable: true,
                changed,
            });
        }

        let mut chunks = Vec::new();
        let scheduling_started = Instant::now();
        let mut inspected_parts = 0_usize;
        let mut inspected_bytes = 0_usize;
        let mut ingestion_truncated_parts = 0_usize;
        while inspected_bytes < byte_limit
            && scheduling_started.elapsed() < scheduling_limit
            && !cancelled.load(Ordering::Acquire)
        {
            let row = if next_cursor.byte_offset > 0 {
                transaction
                    .query_row(
                        "SELECT turn.rowid, turn.source_key, turn.thread_id,
                                turn.turn_index, turn.scope, content.part_index,
                                content.kind, length(content.content), content.truncated,
                                turn.ts_ms
                           FROM turn INDEXED BY turn_session_thread
                           JOIN turn_content AS content ON content.turn_rowid = turn.rowid
                          WHERE turn.environment_key = ?1 AND turn.agent = ?2
                            AND turn.session_id = ?3 AND turn.claim_fence = ?4
                            AND turn.rowid = ?5 AND content.part_index = ?6
                            AND content.kind IN ('user','assistant','thinking','tool_input','tool_result')",
                        params![
                            manifest.identity.environment_key,
                            manifest.identity.agent,
                            manifest.identity.session_id,
                            manifest.published_fence,
                            next_cursor.turn_rowid,
                            next_cursor.part_index,
                        ],
                        deep_row_header,
                    )
                    .optional()?
            } else {
                transaction
                    .query_row(
                        "SELECT turn.rowid, turn.source_key, turn.thread_id,
                                turn.turn_index, turn.scope, content.part_index,
                                content.kind, length(content.content), content.truncated,
                                turn.ts_ms
                           FROM turn INDEXED BY turn_session_thread
                           JOIN turn_content AS content ON content.turn_rowid = turn.rowid
                          WHERE turn.environment_key = ?1 AND turn.agent = ?2
                            AND turn.session_id = ?3 AND turn.claim_fence = ?4
                            AND (turn.thread_id, turn.turn_index, turn.rowid, content.part_index)
                                > (?5, ?6, ?7, ?8)
                            AND content.kind IN ('user','assistant','thinking','tool_input','tool_result')
                          ORDER BY turn.thread_id, turn.turn_index, turn.rowid,
                                   content.part_index
                          LIMIT 1",
                        params![
                            manifest.identity.environment_key,
                            manifest.identity.agent,
                            manifest.identity.session_id,
                            manifest.published_fence,
                            next_cursor.thread_id,
                            next_cursor.turn_index,
                            next_cursor.turn_rowid,
                            next_cursor.part_index,
                        ],
                        deep_row_header,
                    )
                    .optional()?
            };
            let Some(row) = row else {
                next_cursor.complete = true;
                break;
            };
            if next_cursor.byte_offset == 0 {
                next_cursor.thread_id.clone_from(&row.thread_id);
                next_cursor.turn_index = row.turn_index;
                next_cursor.turn_rowid = row.turn_rowid;
                next_cursor.part_index = row.part_index;
                inspected_parts += 1;
                ingestion_truncated_parts += usize::from(row.stored_truncated);
            }
            let remaining = byte_limit - inspected_bytes;
            let chunk_limit = remaining.min(READ_CHUNK_BYTES);
            let start = next_cursor.byte_offset;
            let content = transaction.query_row(
                "SELECT substr(content, ?3, ?4)
                   FROM turn_content
                  WHERE turn_rowid = ?1 AND part_index = ?2",
                params![
                    row.turn_rowid,
                    row.part_index,
                    i64::try_from(start.saturating_add(1))?,
                    i64::try_from(chunk_limit)?,
                ],
                |value| Ok(value.get::<_, Option<Vec<u8>>>(0)?.unwrap_or_default()),
            )?;
            inspected_bytes += content.len();
            next_cursor.byte_offset = next_cursor.byte_offset.saturating_add(content.len());
            chunks.push(DeepContentChunk {
                turn_rowid: row.turn_rowid,
                source_key: row.source_key,
                thread_id: row.thread_id,
                turn_index: row.turn_index,
                scope: row.scope,
                part_index: row.part_index,
                kind: row.kind,
                content,
                byte_offset: start,
                original_bytes: row.original_bytes,
                stored_truncated: row.stored_truncated,
            });
            if next_cursor.byte_offset >= row.original_bytes {
                next_cursor.byte_offset = 0;
            }
            if inspected_bytes >= byte_limit {
                break;
            }
        }
        let complete = next_cursor.complete;
        drop(guard);
        transaction.commit()?;
        *cursor = next_cursor;
        Ok(DeepContentQuantum {
            chunks,
            inspected_parts,
            inspected_bytes,
            ingestion_truncated_parts,
            complete,
            unavailable: false,
            changed: false,
        })
    }

    pub(crate) fn deep_session_version_matches(
        &self,
        manifest: &DeepSessionManifest,
    ) -> Result<(bool, bool)> {
        let connection = self.lock();
        let current = connection
            .query_row(
                "SELECT session.source_generation, evidence.published_fence
                   FROM session
                   JOIN session_evidence AS evidence
                     ON evidence.environment_key = session.environment_key
                    AND evidence.agent = session.agent
                    AND evidence.session_id = session.session_id
                  WHERE session.environment_key = ?1 AND session.agent = ?2
                    AND session.session_id = ?3
                    AND evidence.published_fence IS NOT NULL",
                params![
                    manifest.identity.environment_key,
                    manifest.identity.agent,
                    manifest.identity.session_id,
                ],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        Ok((
            current == Some((manifest.source_generation, manifest.published_fence)),
            current.is_some(),
        ))
    }
}

fn push_manifest_within_budget(
    manifests: &mut Vec<DeepSessionManifest>,
    retained_bytes: &mut usize,
    manifest: DeepSessionManifest,
    session_limit: usize,
    byte_limit: usize,
) -> bool {
    let bytes = manifest_bytes(&manifest);
    if manifests.len() >= session_limit || retained_bytes.saturating_add(bytes) > byte_limit {
        return false;
    }
    *retained_bytes += bytes;
    manifests.push(manifest);
    true
}

fn manifest_bytes(manifest: &DeepSessionManifest) -> usize {
    std::mem::size_of::<DeepSessionManifest>()
        + manifest.identity.environment_key.capacity()
        + manifest.identity.agent.capacity()
        + manifest.identity.session_id.capacity()
        + manifest.session.environment_key.capacity()
        + manifest.session.agent.capacity()
        + manifest.session.session_id.capacity()
        + manifest
            .session
            .wsl_distro
            .as_ref()
            .map_or(0, String::capacity)
        + manifest.session.title.as_ref().map_or(0, String::capacity)
        + manifest.session.repository.capacity()
        + manifest.session.cwd_label.capacity()
        + manifest.session.models.capacity() * std::mem::size_of::<String>()
        + manifest
            .session
            .models
            .iter()
            .map(String::capacity)
            .sum::<usize>()
}

struct DeepRowHeader {
    turn_rowid: i64,
    source_key: String,
    thread_id: String,
    turn_index: i64,
    scope: String,
    part_index: i64,
    kind: String,
    original_bytes: usize,
    stored_truncated: bool,
}

fn deep_row_header(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeepRowHeader> {
    Ok(DeepRowHeader {
        turn_rowid: row.get(0)?,
        source_key: row.get(1)?,
        thread_id: row.get(2)?,
        turn_index: row.get(3)?,
        scope: row.get(4)?,
        part_index: row.get(5)?,
        kind: row.get(6)?,
        original_bytes: usize::try_from(row.get::<_, i64>(7)?).unwrap_or(usize::MAX),
        stored_truncated: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn manifest_read_stops_when_cancelled_before_query() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-deep-manifest-test")).unwrap();
        let cancelled = Arc::new(AtomicBool::new(true));
        let error = store
            .deep_session_manifest(&[], None, cancelled)
            .expect_err("cancelled manifest read must stop");
        assert!(error.to_string().contains("search cancelled"));
    }

    #[test]
    fn manifest_budget_rejects_the_first_row_past_either_limit() {
        let manifest = DeepSessionManifest {
            identity: DeepSessionIdentity {
                environment_key: "native".into(),
                agent: "codex".into(),
                session_id: "one".into(),
            },
            session: SessionSearchResult {
                environment_key: "native".into(),
                agent: "codex".into(),
                session_id: "one".into(),
                wsl_distro: None,
                title: Some("A retained session".into()),
                repository: "repository".into(),
                cwd_label: "repository".into(),
                models: vec!["model".into()],
                updated_at_epoch: Some(1),
            },
            source_generation: 1,
            published_fence: 1,
        };
        let bytes = manifest_bytes(&manifest);
        let mut manifests = Vec::new();
        let mut retained_bytes = 0;
        assert!(push_manifest_within_budget(
            &mut manifests,
            &mut retained_bytes,
            manifest.clone(),
            1,
            bytes,
        ));
        assert!(!push_manifest_within_budget(
            &mut manifests,
            &mut retained_bytes,
            manifest.clone(),
            1,
            usize::MAX,
        ));
        manifests.clear();
        retained_bytes = 0;
        assert!(!push_manifest_within_budget(
            &mut manifests,
            &mut retained_bytes,
            manifest,
            usize::MAX,
            bytes - 1,
        ));
    }
}
