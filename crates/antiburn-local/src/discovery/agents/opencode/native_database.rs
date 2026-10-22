//! Read-only native OpenCode database discovery, records, and fingerprints.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, params_from_iter};
use serde_json::{Value, json};

#[cfg(test)]
mod projection_tests;
mod source_selection;
mod sqlite_budget;
mod titles;
use sqlite_budget::QueryBudget;

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(super) async fn read_session_titles(
    path: PathBuf,
    ids: Vec<String>,
) -> Vec<(String, Option<String>, bool)> {
    let guard = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let cancelled = guard.0.clone();
    tokio::task::spawn_blocking(move || titles::read(&path, &ids, cancelled))
        .await
        .unwrap_or_default()
}

#[cfg(test)]
pub(super) fn query_session_title_by_id(path: &Path, id: &str) -> Option<(Option<String>, bool)> {
    titles::read(path, &[id.to_owned()], Arc::new(AtomicBool::new(false)))
        .pop()
        .map(|(_, title, v2)| (title, v2))
}

use crate::discovery::scanner::{AgentKind, TitleSource};
use crate::discovery::{FORK_OBSERVATION_KEY, ForkObservation, SessionLog, SessionSource};

use super::{
    DbSourceRead, MAX_DB_FORK_CANDIDATES, MAX_DB_FORK_RECORD_BYTES, MessageRecord, PartRecord,
    SessionRecord, build_session_clusters, database_paths_for_root, group_messages_by_session,
    group_parts_by_session, merge_json_object, normalize_db_epoch, normalize_opencode_visible_text,
    opencode_fork_observation, opencode_fork_parent_title, parse_epoch_from_json_value, parse_json,
    render_session_logs, sql_in_placeholders, strict_opencode_visible_prefix, valid_session_id,
};

#[derive(Debug, Default)]
pub(super) struct DbRecords {
    pub(super) sessions: Vec<(String, SessionRecord)>,
    pub(super) messages: Vec<MessageRecord>,
    pub(super) parts: Vec<PartRecord>,
}

struct DbSessionRowData {
    session_id: String,
    parent_session_id: Option<String>,
    directory: Option<String>,
    title: Option<String>,
    time_created: Option<i64>,
    time_updated: Option<i64>,
    raw_json: Option<String>,
}

struct DbSessionMetadataRow {
    session_id: String,
    directory: Option<String>,
    title: Option<String>,
}

#[cfg(test)]
pub(super) fn query_recent_db_session_sources(root: &Path, cutoff: i64) -> DbSourceRead {
    query_recent_db_session_sources_cancellable(root, cutoff, Arc::new(AtomicBool::new(false)))
}

pub(super) async fn read_recent_db_session_sources(
    root: PathBuf,
    cutoff: i64,
) -> Result<DbSourceRead, tokio::task::JoinError> {
    let guard = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let cancelled = guard.0.clone();
    tokio::task::spawn_blocking(move || {
        query_recent_db_session_sources_cancellable(&root, cutoff, cancelled)
    })
    .await
}

pub(super) async fn prefer_v2_session_sources(roots: &[PathBuf], logs: &mut Vec<SessionLog>) {
    let mut activity = HashMap::<PathBuf, HashMap<String, i64>>::new();
    for log in logs.iter() {
        if let SessionSource::ProviderDb {
            db_path,
            session_id,
            source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
            ..
        } = &log.source
            && let Some(updated) = log.updated_at
        {
            activity
                .entry(db_path.clone())
                .or_default()
                .entry(session_id.clone())
                .and_modify(|latest| *latest = (*latest).max(updated))
                .or_insert(updated);
        }
    }
    let guard = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let mut pending = std::mem::take(logs).into_iter();
    loop {
        let batch: Vec<_> = pending.by_ref().take(256).collect();
        if batch.is_empty() {
            break;
        }
        let ids: HashSet<_> = batch
            .iter()
            .filter_map(source_selection::candidate_id)
            .map(str::to_owned)
            .collect();
        if ids.is_empty() {
            logs.extend(batch);
            continue;
        }
        let roots = roots.to_vec();
        let cancelled = guard.0.clone();
        let result =
            tokio::task::spawn_blocking(move || source_selection::select(&roots, ids, cancelled))
                .await;
        match result {
            Ok(Ok(selected)) => {
                for mut log in batch {
                    if let Some(id) = source_selection::candidate_id(&log)
                        && let Some(selection) = selected.get(id)
                    {
                        let Some(source) = selection else { continue };
                        let same_source = matches!(
                            (&log.source, &source.source),
                            (SessionSource::ProviderDb { db_path, session_id, source_format, .. },
                             SessionSource::ProviderDb { db_path: selected_path, session_id: selected_id, source_format: selected_format, .. })
                            if db_path == selected_path && session_id == selected_id && source_format == selected_format
                        );
                        if !same_source {
                            log = source.clone();
                        }
                        if let SessionSource::ProviderDb {
                            db_path,
                            session_id,
                            ..
                        } = &source.source
                            && let Some(updated) = activity
                                .get(db_path)
                                .and_then(|sessions| sessions.get(session_id))
                        {
                            log.updated_at = Some(*updated);
                        }
                    }
                    logs.push(log);
                }
            }
            result => {
                let stage = match result {
                    Ok(Err(stage)) => stage,
                    _ => "source_selection_worker",
                };
                tracing::warn!(event = "opencode_sqlite_source_selection_failed", stage);
                logs.extend(
                    batch
                        .into_iter()
                        .filter(|log| source_selection::candidate_id(log).is_none()),
                );
            }
        }
    }
}

fn query_recent_db_session_sources_cancellable(
    root: &Path,
    cutoff: i64,
    cancelled: Arc<AtomicBool>,
) -> DbSourceRead {
    let paths = match database_paths_for_root(root) {
        Ok(paths) => paths,
        Err(stage) => return DbSourceRead::Failed(stage),
    };
    let mut logs = Vec::new();
    let mut outcome = DbSourceRead::NotPresent;
    let mut ready = false;
    for path in paths {
        if cancelled.load(Ordering::Acquire) {
            return DbSourceRead::Failed("cancelled");
        }
        match query_recent_db_session_sources_at_path(&path, cutoff, cancelled.clone()) {
            DbSourceRead::Ready(found) => {
                ready = true;
                logs.extend(found);
            }
            DbSourceRead::NotPresent => {}
            DbSourceRead::UnsupportedSchema => {
                tracing::warn!(event = "opencode_sqlite_schema_unsupported");
                outcome = DbSourceRead::UnsupportedSchema;
            }
            DbSourceRead::Failed(stage) => {
                tracing::warn!(event = "opencode_sqlite_discovery_failed", stage);
                outcome = DbSourceRead::Failed(stage);
            }
        }
    }
    if ready {
        crate::discovery::dedupe_environment_sessions(&mut logs);
        DbSourceRead::Ready(logs)
    } else {
        outcome
    }
}

fn query_recent_db_session_sources_at_path(
    db_path: &Path,
    cutoff: i64,
    cancelled: Arc<AtomicBool>,
) -> DbSourceRead {
    match std::fs::metadata(db_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return DbSourceRead::NotPresent;
        }
        Err(_) => return DbSourceRead::Failed("metadata"),
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    let Ok(conn) = conn else {
        return DbSourceRead::Failed("open");
    };
    if conn.busy_timeout(Duration::from_millis(250)).is_err() {
        return DbSourceRead::Failed("busy_timeout");
    }
    let budget = QueryBudget::new(&conn, cancelled);
    let result = query_recent_db_session_sources_connection(&conn, db_path, cutoff);
    if let Err(stage) = budget.check() {
        return DbSourceRead::Failed(stage);
    }
    match result {
        DbSourceRead::Failed(stage) => DbSourceRead::Failed(budget.failure(stage)),
        result => result,
    }
}

fn query_recent_db_session_sources_connection(
    conn: &Connection,
    db_path: &Path,
    cutoff: i64,
) -> DbSourceRead {
    let Ok(conn) = conn.unchecked_transaction() else {
        return DbSourceRead::Failed("snapshot");
    };
    let schema = match opencode_sqlite_schema(&conn) {
        Ok(schema) => schema,
        Err(_) => return DbSourceRead::Failed("schema"),
    };
    let mut out = if schema.has_v2() {
        match query_recent_v2_session_sources(&conn, db_path, cutoff) {
            Ok(logs) => logs,
            Err(stage) => return DbSourceRead::Failed(stage),
        }
    } else {
        Vec::new()
    };
    if !schema.has_v1() {
        return if schema.has_v2() {
            DbSourceRead::Ready(out)
        } else {
            DbSourceRead::UnsupportedSchema
        };
    }
    let recent_session_ids = recent_db_session_ids(&conn, cutoff);
    if recent_session_ids.is_empty() {
        return DbSourceRead::Ready(out);
    }
    let cluster_session_ids = db_session_ids_with_ancestors(&conn, &recent_session_ids);
    let mut sessions: HashMap<String, SessionRecord> =
        fetch_db_sessions_light(&conn, db_path, &cluster_session_ids)
            .into_iter()
            .collect();
    if schema.has_v2() {
        let mut migrated = Vec::new();
        for session_id in sessions.keys() {
            match conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id = ?1)",
                [session_id],
                |row| row.get::<_, bool>(0),
            ) {
                Ok(true) => migrated.push(session_id.clone()),
                Ok(false) => {}
                Err(_) => return DbSourceRead::Failed("source_selection"),
            }
        }
        for session_id in migrated {
            sessions.remove(&session_id);
        }
    }
    let activity_by_session = fetch_db_activity_by_session(&conn, &cluster_session_ids);
    let clusters = build_session_clusters(&sessions, &BTreeMap::new(), &BTreeMap::new());

    for cluster in clusters {
        let mut max_updated = 0i64;
        for session_id in &cluster.session_ids {
            if let Some(record) = sessions.get(session_id)
                && let Some(ts) = record
                    .updated_at
                    .or(record.created_at)
                    .or(record.file_mtime)
            {
                max_updated = max_updated.max(ts);
            }
            if let Some(ts) = activity_by_session.get(session_id) {
                max_updated = max_updated.max(*ts);
            }
        }
        if max_updated == 0 || max_updated < cutoff {
            continue;
        }
        out.push(SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: db_path.to_path_buf(),
                session_id: cluster.root_session_id,
                source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV1),
            },
            updated_at: Some(max_updated),
        });
    }
    out.sort_by(|a, b| {
        b.updated_at
            .unwrap_or_default()
            .cmp(&a.updated_at.unwrap_or_default())
            .then(a.source_label().cmp(&b.source_label()))
    });
    DbSourceRead::Ready(out)
}

const MAX_V2_RECENT_SESSIONS: usize = 1000;
const MAX_V2_PARENT_DEPTH: usize = 32;
const MAX_V2_ADMISSION_BYTES: usize = 1024 * 1024;

fn query_recent_v2_session_sources(
    conn: &Connection,
    db_path: &Path,
    cutoff: i64,
) -> Result<Vec<SessionLog>, &'static str> {
    let mut statement = conn.prepare(
        "SELECT CASE WHEN length(CAST(s.id AS BLOB)) <= 256 THEN s.id END,
                MAX(s.time_created, s.time_updated, COALESCE(MAX(MAX(sm.time_created, sm.time_updated)), 0)),
                CASE WHEN length(CAST(s.parent_id AS BLOB)) <= 256 THEN s.parent_id END,
                s.parent_id IS NOT NULL AND length(CAST(s.parent_id AS BLOB)) > 256
         FROM session_v2 s
         LEFT JOIN session_message sm ON sm.session_id = s.id
         GROUP BY s.id
         HAVING MAX(s.time_created, s.time_updated, COALESCE(MAX(MAX(sm.time_created, sm.time_updated)), 0)) >= ?1
         ORDER BY MAX(s.time_created, s.time_updated, COALESCE(MAX(MAX(sm.time_created, sm.time_updated)), 0)) DESC, s.id
         LIMIT ?2",
    ).map_err(|_| "query")?;
    let rows = statement
        .query_map(
            params![cutoff.saturating_mul(1000), MAX_V2_RECENT_SESSIONS as i64],
            |row| {
                if row.get::<_, bool>(3)? {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|_| "query")?;
    let recent = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "row")?;
    let mut roots: BTreeMap<String, i64> = BTreeMap::new();
    for (session_id, activity, parent_id) in recent {
        if !valid_opencode_v2_session_id(&session_id) {
            continue;
        }
        let root_id = v2_root_session_id(conn, session_id, parent_id)?;
        roots
            .entry(root_id)
            .and_modify(|latest| {
                *latest = (*latest).max(normalize_db_epoch(Some(activity)).unwrap_or_default())
            })
            .or_insert_with(|| normalize_db_epoch(Some(activity)).unwrap_or_default());
    }
    let mut logs: Vec<_> = roots
        .into_iter()
        .filter(|(_, latest)| *latest > 0)
        .map(|(session_id, latest)| SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::OpenCode,
            source: SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path: db_path.to_path_buf(),
                session_id,
                source_format: Some(crate::analysis::SourceFormat::OpenCodeSqliteV2),
            },
            updated_at: Some(latest),
        })
        .collect();
    logs.sort_by(|a, b| {
        b.updated_at
            .unwrap_or_default()
            .cmp(&a.updated_at.unwrap_or_default())
            .then(a.source_label().cmp(&b.source_label()))
    });
    Ok(logs)
}

fn valid_opencode_v2_session_id(id: &str) -> bool {
    id.starts_with("ses_") && valid_session_id(id)
}

fn v2_root_session_id(
    conn: &Connection,
    mut session_id: String,
    mut parent_id: Option<String>,
) -> Result<String, &'static str> {
    let mut seen = HashSet::from([session_id.clone()]);
    for _ in 0..MAX_V2_PARENT_DEPTH {
        let Some(parent) = parent_id else {
            return Ok(session_id);
        };
        if !valid_opencode_v2_session_id(&parent) {
            return Err("hierarchy");
        }
        if !seen.insert(parent.clone()) {
            return Err("hierarchy");
        }
        let row = conn.query_row(
            "SELECT CASE WHEN length(CAST(parent_id AS BLOB)) <= 256 THEN parent_id END,
                    parent_id IS NOT NULL AND length(CAST(parent_id AS BLOB)) > 256
               FROM session_v2 WHERE id = ?1 LIMIT 1",
            params![parent],
            |row| {
                if row.get::<_, bool>(1)? {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                row.get::<_, Option<String>>(0)
            },
        );
        let next_parent = row.map_err(|_| "hierarchy")?;
        session_id = parent;
        parent_id = next_parent;
    }
    if parent_id.is_some() {
        return Err("hierarchy");
    }
    Ok(session_id)
}

pub async fn render_db_session(db_path: PathBuf, root_session_id: String) -> Option<String> {
    tokio::task::spawn_blocking(move || render_db_session_blocking(&db_path, &root_session_id))
        .await
        .ok()
        .flatten()
}

/// Find a database-backed fork parent without rendering the session transcript.
pub async fn db_fork_parent(db_path: PathBuf, session_id: String) -> Option<String> {
    tokio::task::spawn_blocking(move || db_fork_parent_blocking(&db_path, &session_id))
        .await
        .ok()
        .flatten()
}

pub async fn db_session_metadata(
    db_path: PathBuf,
    root_session_id: String,
    source_format: crate::analysis::SourceFormat,
) -> Option<crate::discovery::scanner::SessionMetadata> {
    tokio::task::spawn_blocking(move || {
        db_session_metadata_blocking(&db_path, &root_session_id, source_format)
    })
    .await
    .ok()
    .flatten()
}

fn render_db_session_blocking(db_path: &Path, root_session_id: &str) -> Option<String> {
    let mut records = query_db_records_for_root(db_path, root_session_id)?;
    if let Some(observation) = infer_db_fork_observation(db_path, root_session_id, &records)
        && let Some((_, session)) = records
            .sessions
            .iter_mut()
            .find(|(session_id, _)| session_id == root_session_id)
    {
        session.raw[FORK_OBSERVATION_KEY] =
            serde_json::to_value(observation).unwrap_or(Value::Null);
    }
    let logs = render_session_logs(
        0,
        records.sessions.into_iter().collect(),
        group_messages_by_session(records.messages),
        group_parts_by_session(records.parts),
    );
    let expected_label = format!("opencode:{root_session_id}");
    logs.into_iter().find_map(|log| {
        if log.source_label() != expected_label {
            return None;
        }
        match log.source {
            SessionSource::Inline { content, .. } => Some(content),
            SessionSource::File(_) | SessionSource::ProviderDb { .. } => None,
        }
    })
}

pub(super) fn db_fork_parent_blocking(db_path: &Path, session_id: &str) -> Option<String> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _budget = QueryBudget::new(&conn, Arc::new(AtomicBool::new(false)));
    conn.execute_batch("BEGIN").ok()?;
    match db_session_source_format_connection(&conn, session_id)? {
        crate::analysis::SourceFormat::OpenCodeSqliteV2 => {
            return conn
                .query_row(
                    "SELECT CASE WHEN length(CAST(fork_session_id AS BLOB)) <= 256 THEN fork_session_id END FROM session_v2 WHERE id = ?1",
                    [session_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .ok()?
                .filter(|parent| valid_opencode_v2_session_id(parent) && parent != session_id);
        }
        crate::analysis::SourceFormat::OpenCodeSqliteV1 => {}
        _ => return None,
    }
    let (directory, title, child_created) = conn
        .query_row(
            "SELECT directory, title, time_created FROM session WHERE id = ?1 LIMIT 1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2).ok().flatten(),
                ))
            },
        )
        .ok()?;
    let parent_title = opencode_fork_parent_title(&title)?;
    let candidate_count = conn
        .query_row(
            "SELECT COUNT(*) FROM session
              WHERE id != ?1 AND directory = ?2 AND title = ?3 AND parent_id IS NULL",
            params![session_id, directory, parent_title],
            |row| row.get::<_, i64>(0),
        )
        .ok()?;
    if candidate_count == 0 || candidate_count > MAX_DB_FORK_CANDIDATES {
        return None;
    }

    let visible_sql = "WITH bounded AS (
             SELECT CASE WHEN length(CAST(message.data AS BLOB)) <= ?2
                               AND json_valid(message.data)
                         THEN message.data END AS message_data,
                     length(CAST(message.data AS BLOB)) AS message_bytes,
                     json_valid(message.data) AS message_valid,
                     CASE WHEN length(CAST(part.data AS BLOB)) <= ?2
                               AND json_valid(part.data)
                         THEN part.data END AS part_data,
                     length(CAST(part.data AS BLOB)) AS part_bytes,
                     json_valid(part.data) AS part_valid,
                    message.time_created AS message_created,
                    message.time_updated AS message_updated,
                    message.id AS message_id,
                    part.time_created AS part_created,
                    part.time_updated AS part_updated,
                    part.id AS part_id
               FROM message JOIN part ON part.message_id = message.id
              WHERE message.session_id = ?1
         )
         SELECT CASE WHEN json_type(message_data, '$.role') = 'text'
                     THEN json_extract(message_data, '$.role') END,
                 message_bytes,
                 message_valid,
                 CASE WHEN json_type(part_data, '$.type') = 'text'
                      THEN json_extract(part_data, '$.type') END,
                 CASE WHEN json_extract(part_data, '$.type') = 'text'
                            AND json_type(part_data, '$.text') = 'text'
                      THEN json_extract(part_data, '$.text') END,
                 part_bytes,
                 part_valid
           FROM bounded
          ORDER BY COALESCE(message_created, message_updated, 0), message_id,
                   COALESCE(part_created, part_updated, 0), part_id";
    let mut parent_items = conn.prepare(visible_sql).ok()?;
    let mut child_items = conn.prepare(visible_sql).ok()?;
    let mut candidates = conn
        .prepare(
            "SELECT id, time_created FROM session
              WHERE id != ?1 AND directory = ?2 AND title = ?3 AND parent_id IS NULL",
        )
        .ok()?;
    let rows = candidates
        .query_map(params![session_id, directory, parent_title], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1).ok().flatten(),
            ))
        })
        .ok()?;

    let mut best: Option<(String, usize)> = None;
    let mut best_is_tied = false;
    for (parent_session_id, parent_created) in rows.flatten() {
        if parent_created
            .zip(child_created)
            .is_some_and(|(parent, child)| parent > child)
        {
            continue;
        }
        let Some(inherited) = db_visible_prefix_len(
            &mut parent_items,
            &parent_session_id,
            &mut child_items,
            session_id,
            MAX_DB_FORK_RECORD_BYTES,
        ) else {
            continue;
        };
        match &best {
            Some((_, best_inherited)) if inherited < *best_inherited => {}
            Some((_, best_inherited)) if inherited == *best_inherited => best_is_tied = true,
            _ => {
                best = Some((parent_session_id, inherited));
                best_is_tied = false;
            }
        }
    }

    if best_is_tied {
        return None;
    }
    best.map(|(parent_session_id, _)| parent_session_id)
}

fn db_visible_prefix_len(
    parent_statement: &mut rusqlite::Statement<'_>,
    parent_session_id: &str,
    child_statement: &mut rusqlite::Statement<'_>,
    child_session_id: &str,
    max_record_bytes: i64,
) -> Option<usize> {
    let mut parent_rows = parent_statement
        .query(params![parent_session_id, max_record_bytes])
        .ok()?;
    let mut child_rows = child_statement
        .query(params![child_session_id, max_record_bytes])
        .ok()?;
    let mut inherited = 0;

    while let Some(parent) = next_db_visible_item(&mut parent_rows)? {
        if next_db_visible_item(&mut child_rows)? != Some(parent) {
            return None;
        }
        inherited += 1;
    }
    (inherited >= 2 && next_db_visible_item(&mut child_rows)?.is_some()).then_some(inherited)
}

fn next_db_visible_item(rows: &mut rusqlite::Rows<'_>) -> Option<Option<(String, String)>> {
    while let Some(row) = rows.next().ok()? {
        let message_bytes = row.get::<_, Option<i64>>(1).ok().flatten().unwrap_or(0);
        let message_valid = row.get::<_, bool>(2).ok()?;
        let part_bytes = row.get::<_, Option<i64>>(5).ok().flatten().unwrap_or(0);
        let part_valid = row.get::<_, bool>(6).ok()?;
        if message_bytes > MAX_DB_FORK_RECORD_BYTES || part_bytes > MAX_DB_FORK_RECORD_BYTES {
            return None;
        }
        if !message_valid || !part_valid {
            return None;
        }
        let role = row.get::<_, Option<String>>(0).ok().flatten();
        let part_type = row.get::<_, Option<String>>(3).ok().flatten();
        let text = row.get::<_, Option<String>>(4).ok().flatten();
        if let (Some(role), Some("text"), Some(text)) = (role, part_type.as_deref(), text)
            && matches!(role.as_str(), "user" | "assistant")
        {
            let text = normalize_opencode_visible_text(&text);
            if !text.is_empty() {
                return Some(Some((role, text)));
            }
        }
    }
    Some(None)
}

pub(super) fn db_session_metadata_blocking(
    db_path: &Path,
    root_session_id: &str,
    source_format: crate::analysis::SourceFormat,
) -> Option<crate::discovery::scanner::SessionMetadata> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _budget = QueryBudget::new(&conn, Arc::new(AtomicBool::new(false)));
    if db_session_source_format_connection(&conn, root_session_id)? != source_format {
        return None;
    }
    let row = match source_format {
        crate::analysis::SourceFormat::OpenCodeSqliteV2
            if valid_opencode_v2_session_id(root_session_id) =>
        {
            let (selected, byte_length) = bounded_v2_columns(
                &conn,
                "session_v2",
                &["id", "directory", "title"],
                MAX_V2_ADMISSION_BYTES as u64,
            );
            conn.query_row(
                &format!("SELECT {selected}, {byte_length} FROM session_v2 WHERE id = ?1 LIMIT 1"),
                params![root_session_id],
                |row| {
                    if row.get::<_, u64>(3)? > MAX_V2_ADMISSION_BYTES as u64 {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    Ok(DbSessionMetadataRow {
                        session_id: row.get(0)?,
                        directory: row.get(1).ok(),
                        title: row
                            .get::<_, Option<String>>(2)
                            .ok()
                            .flatten()
                            .and_then(|title| {
                                let title = title.trim().to_owned();
                                (!title.is_empty()).then_some(title)
                            }),
                    })
                },
            )
            .ok()?
        }
        crate::analysis::SourceFormat::OpenCodeSqliteV1 => {
            query_db_session_metadata_row(&conn, root_session_id)?
        }
        _ => return None,
    };
    let title_source = row.title.as_ref().map(|_| TitleSource::Explicit);
    Some(crate::discovery::scanner::SessionMetadata {
        session_id: Some(row.session_id),
        cwd: row.directory,
        agent_type: Some(AgentKind::OpenCode),
        title: row.title,
        title_source,
    })
}

fn query_db_records_for_root(db_path: &Path, root_session_id: &str) -> Option<DbRecords> {
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    query_db_records_for_root_connection(&conn, db_path, root_session_id)
}

pub(super) fn query_db_records_for_root_connection(
    conn: &Connection,
    db_path: &Path,
    root_session_id: &str,
) -> Option<DbRecords> {
    let cluster_session_ids = db_session_ids_for_root(conn, root_session_id);
    if cluster_session_ids.is_empty() {
        return None;
    }
    Some(DbRecords {
        sessions: fetch_db_sessions(conn, db_path, &cluster_session_ids),
        messages: fetch_db_messages(conn, db_path, &cluster_session_ids),
        parts: fetch_db_parts(conn, db_path, &cluster_session_ids),
    })
}

fn infer_db_fork_observation(
    db_path: &Path,
    child_session_id: &str,
    child_records: &DbRecords,
) -> Option<ForkObservation> {
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    infer_db_fork_observation_connection(&conn, db_path, child_session_id, child_records)
}

fn infer_db_fork_observation_connection(
    conn: &Connection,
    db_path: &Path,
    child_session_id: &str,
    child_records: &DbRecords,
) -> Option<ForkObservation> {
    let (_, child_session) = child_records
        .sessions
        .iter()
        .find(|(session_id, _)| session_id == child_session_id)?;
    let parent_title = child_session
        .title
        .as_deref()
        .and_then(opencode_fork_parent_title)?;
    let child_items = opencode_record_visible_items(child_records, child_session_id);
    if child_items.len() < 3 {
        return None;
    }

    let mut stmt = conn
        .prepare(
            "SELECT id FROM session WHERE id != ?1 AND directory = ?2 AND title = ?3 AND parent_id IS NULL LIMIT 101",
        )
        .ok()?;
    let rows = stmt
        .query_map(
            params![child_session_id, child_session.directory, parent_title],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    let mut matches = rows
        .flatten()
        .filter_map(|parent_session_id| {
            let records = query_db_records_for_root_connection(conn, db_path, &parent_session_id)?;
            let (_, parent_session) = records
                .sessions
                .iter()
                .find(|(session_id, _)| session_id == &parent_session_id)?;
            if parent_session
                .created_at
                .zip(child_session.created_at)
                .is_some_and(|(parent, child)| parent > child)
            {
                return None;
            }
            let items = opencode_record_visible_items(&records, &parent_session_id);
            strict_opencode_visible_prefix(&items, &child_items)
                .then_some((parent_session_id, items.len()))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(_, inherited)| std::cmp::Reverse(*inherited));
    let (parent_session_id, inherited) = matches.first()?;
    if matches.get(1).is_some_and(|(_, other)| other == inherited) {
        return None;
    }
    Some(opencode_fork_observation(parent_session_id, *inherited))
}

fn opencode_record_visible_items(
    records: &DbRecords,
    root_session_id: &str,
) -> Vec<(String, String)> {
    let mut messages = records
        .messages
        .iter()
        .filter(|message| message.session_id == root_session_id)
        .collect::<Vec<_>>();
    messages.sort_by(|a, b| {
        a.created_at
            .unwrap_or_default()
            .cmp(&b.created_at.unwrap_or_default())
            .then(a.id.cmp(&b.id))
    });
    let mut out = Vec::new();
    for message in messages {
        let Some(role) = message
            .role
            .as_deref()
            .filter(|role| matches!(*role, "user" | "assistant"))
        else {
            continue;
        };
        let mut parts = records
            .parts
            .iter()
            .filter(|part| part.message_id.as_deref() == Some(message.id.as_str()))
            .filter(|part| part.part_type.as_deref() == Some("text"))
            .collect::<Vec<_>>();
        parts.sort_by(|a, b| {
            a.created_at
                .unwrap_or_default()
                .cmp(&b.created_at.unwrap_or_default())
                .then(a.id.cmp(&b.id))
        });
        for part in parts {
            let Some(text) = part.raw.get("text").and_then(Value::as_str) else {
                continue;
            };
            let text = normalize_opencode_visible_text(text);
            if !text.is_empty() {
                out.push((role.to_string(), text));
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OpenCodeSqliteSchema {
    V1,
    V2,
    Mixed,
    Unsupported,
    Malformed,
}

impl OpenCodeSqliteSchema {
    pub(super) fn has_v1(self) -> bool {
        matches!(self, Self::V1 | Self::Mixed)
    }

    pub(super) fn has_v2(self) -> bool {
        matches!(self, Self::V2 | Self::Mixed)
    }
}

pub(super) fn opencode_sqlite_schema(conn: &Connection) -> rusqlite::Result<OpenCodeSqliteSchema> {
    let table_exists = |table: &str| {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            params![table],
            |row| row.get::<_, bool>(0),
        )
    };
    let v1_marker = table_exists("session")?;
    let v1 = v1_marker && db_table_has_column(conn, "session", "id");
    // A V2 marker blocks V1 fallback, even when the V2 schema is incomplete.
    if table_exists("session_v2")? || table_exists("session_message")? {
        return Ok(
            if crate::analysis::opencode_v2_schema::is_supported(conn)? {
                if v1 {
                    OpenCodeSqliteSchema::Mixed
                } else {
                    OpenCodeSqliteSchema::V2
                }
            } else {
                OpenCodeSqliteSchema::Malformed
            },
        );
    }
    Ok(if v1 {
        OpenCodeSqliteSchema::V1
    } else if v1_marker {
        OpenCodeSqliteSchema::Malformed
    } else {
        OpenCodeSqliteSchema::Unsupported
    })
}

pub(super) fn db_session_source_format_connection(
    conn: &Connection,
    session_id: &str,
) -> Option<crate::analysis::SourceFormat> {
    let schema = opencode_sqlite_schema(conn).ok()?;
    for (supported, table, format) in [
        (
            schema.has_v2(),
            "session_v2",
            crate::analysis::SourceFormat::OpenCodeSqliteV2,
        ),
        (
            schema.has_v1(),
            "session",
            crate::analysis::SourceFormat::OpenCodeSqliteV1,
        ),
    ] {
        if supported
            && conn
                .query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
                    [session_id],
                    |row| row.get::<_, bool>(0),
                )
                .ok()?
        {
            return Some(format);
        }
    }
    None
}

pub(super) fn db_session_source_format(
    db_path: &Path,
    session_id: &str,
) -> Option<crate::analysis::SourceFormat> {
    if !db_path.exists() || !valid_session_id(session_id) {
        return None;
    }
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    db_session_source_format_connection(&conn, session_id)
}

pub(super) fn db_session_fingerprint_blocking(
    db_path: &Path,
    root_session_id: &str,
) -> Option<(u64, u64)> {
    if !db_path.exists() {
        return None;
    }
    let expected_identity = database_identity_at_path(db_path)?;
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    verify_database_identity(&conn, &expected_identity)?;
    db_session_fingerprint_cancellable_with_identity(
        &conn,
        root_session_id,
        &expected_identity,
        &|| false,
    )
}

/// Read only session metadata and indexed record frontiers. Old payload edits
/// require strict reconciliation even when this revision stays equal.
/// Bind the admission revision to the file identity captured before SQLite opened the file.
pub fn db_session_admission_revision_connection_with_identity(
    conn: &Connection,
    root_session_id: &str,
    expected_identity: &str,
) -> Option<String> {
    verify_database_identity(conn, expected_identity)?;
    let snapshot = if conn.is_autocommit() {
        Some(conn.unchecked_transaction().ok()?)
    } else {
        None
    };
    match db_session_source_format_connection(conn, root_session_id)? {
        crate::analysis::SourceFormat::OpenCodeSqliteV2 => {
            let budget = QueryBudget::new(conn, Arc::new(AtomicBool::new(false)));
            let revision =
                db_session_v2_admission_revision(conn, root_session_id, expected_identity);
            budget.check().ok()?;
            verify_database_identity(conn, expected_identity)?;
            return revision;
        }
        crate::analysis::SourceFormat::OpenCodeSqliteV1 => {}
        _ => return None,
    }
    let cluster = db_v1_cluster_cte(conn, Some(257))?;
    let mut statement = conn
        .prepare(&format!(
            "{cluster} SELECT id FROM cluster ORDER BY id <> ?1, id"
        ))
        .ok()?;
    let ids = statement
        .query_map([root_session_id], |row| row.get::<_, String>(0))
        .ok()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .ok()?;
    if ids.is_empty() {
        return None;
    }
    let mut hash = DbFingerprintHash::new();
    // Strict reconciliation covers records outside these bounded frontiers.
    hash.write_bytes(&[u8::from(ids.len() > 256)]);
    for id in ids.into_iter().take(256) {
        for (table, predicate, columns, limit) in [
            (
                "session",
                "id",
                &[
                    "id",
                    "parent_id",
                    "directory",
                    "title",
                    "time_created",
                    "time_updated",
                ][..],
                1,
            ),
            (
                "message",
                "session_id",
                &["id", "session_id", "time_created", "time_updated"][..],
                8,
            ),
            (
                "part",
                "session_id",
                &[
                    "id",
                    "message_id",
                    "session_id",
                    "time_created",
                    "time_updated",
                ][..],
                8,
            ),
        ] {
            hash.write_bytes(table.as_bytes());
            let selected = columns
                .iter()
                .map(|column| {
                    if db_table_has_column(conn, table, column) {
                        (*column).to_owned()
                    } else {
                        "NULL".to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let mut rows = conn.prepare(&format!(
                "SELECT rowid, {selected} FROM {table} WHERE {predicate} = ?1 ORDER BY rowid DESC LIMIT {limit}"
            )).ok()?;
            let mut rows = rows.query([&id]).ok()?;
            while let Some(row) = rows.next().ok()? {
                for index in 0..columns.len() + 1 {
                    hash.write_value(row.get_ref(index).ok()?);
                }
            }
        }
        if ["aggregate_id", "seq", "owner_id"]
            .iter()
            .all(|column| db_table_has_column(conn, "event_sequence", column))
        {
            let mut statement = conn.prepare(
                "SELECT aggregate_id, seq, owner_id FROM event_sequence WHERE aggregate_id = ?1 LIMIT 1"
            ).ok()?;
            let mut rows = statement.query([&id]).ok()?;
            if let Some(row) = rows.next().ok()? {
                for index in 0..3 {
                    hash.write_value(row.get_ref(index).ok()?);
                }
            }
        }
    }
    drop(snapshot);
    verify_database_identity(conn, expected_identity)?;
    Some(format!("scope-v1:{:016x}", hash.finish()))
}

/// Hash one SQLite snapshot. The caller checks the current scoped revision
/// after the snapshot closes and before it publishes the hash.
pub fn db_session_fingerprint_snapshot(
    db_path: &Path,
    root_session_id: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<(String, (u64, u64))> {
    let expected_identity = database_identity_at_path(db_path)?;
    db_session_fingerprint_snapshot_with_identity(
        db_path,
        root_session_id,
        &expected_identity,
        cancelled,
    )
}

pub fn db_session_fingerprint_snapshot_with_identity(
    db_path: &Path,
    root_session_id: &str,
    expected_identity: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<(String, (u64, u64))> {
    verify_path_identity(db_path, expected_identity)?;
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    verify_database_identity(&conn, expected_identity)?;
    let snapshot = conn.unchecked_transaction().ok()?;
    let revision = db_session_admission_revision_connection_with_identity(
        &snapshot,
        root_session_id,
        expected_identity,
    )?;
    let fingerprint = db_session_fingerprint_cancellable_with_identity(
        &snapshot,
        root_session_id,
        expected_identity,
        cancelled,
    )?;
    verify_database_identity(&conn, expected_identity)?;
    Some((revision, fingerprint))
}

pub fn db_session_fingerprint_cancellable_with_identity(
    conn: &Connection,
    root_session_id: &str,
    expected_identity: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<(u64, u64)> {
    verify_database_identity(conn, expected_identity)?;
    match db_session_source_format_connection(conn, root_session_id)? {
        crate::analysis::SourceFormat::OpenCodeSqliteV2 => {
            let budget = QueryBudget::strict(conn);
            let fingerprint = db_session_v2_fingerprint_cancellable_with_identity(
                conn,
                root_session_id,
                expected_identity,
                cancelled,
            );
            budget.check().ok()?;
            verify_database_identity(conn, expected_identity)?;
            fingerprint
        }
        crate::analysis::SourceFormat::OpenCodeSqliteV1 => {
            let fingerprint =
                db_session_fingerprint_v1_cancellable(conn, root_session_id, cancelled);
            verify_database_identity(conn, expected_identity)?;
            fingerprint
        }
        _ => None,
    }
}

fn db_session_fingerprint_v1_cancellable(
    conn: &Connection,
    root_session_id: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<(u64, u64)> {
    let mut trace = DbFingerprintTrace::start("full");
    let cluster = db_v1_cluster_cte(conn, None)?;
    let mut hash = DbFingerprintHash::new();
    hash.write_bytes(root_session_id.as_bytes());
    let mut rows = 0_u64;
    for (table, join, columns) in [
        (
            "session",
            "session.id = cluster.id",
            &[
                "id",
                "parent_id",
                "directory",
                "title",
                "time_created",
                "time_updated",
                "data",
            ][..],
        ),
        (
            "message",
            "message.session_id = cluster.id",
            &["id", "session_id", "time_created", "time_updated", "data"][..],
        ),
        (
            "part",
            "part.session_id = cluster.id",
            &[
                "id",
                "message_id",
                "session_id",
                "time_created",
                "time_updated",
                "data",
            ][..],
        ),
    ] {
        let mut table_trace = DbFingerprintTrace::start(table);
        hash.write_bytes(table.as_bytes());
        let selected = columns
            .iter()
            .map(|column| {
                if db_table_has_column(conn, table, column) {
                    format!("{table}.{column}")
                } else {
                    "NULL".to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let order = if db_table_has_column(conn, table, "id") {
            format!("{table}.id")
        } else {
            format!("{table}.rowid")
        };
        let sql = format!(
            "{cluster} SELECT {selected} FROM {table} JOIN cluster ON {join} ORDER BY {order}"
        );
        let mut statement = conn.prepare(&sql).ok()?;
        let mut query = statement.query(params![root_session_id]).ok()?;
        while let Some(row) = query.next().ok()? {
            if cancelled() {
                return None;
            }
            rows = rows.saturating_add(1);
            trace.rows = rows;
            table_trace.rows = table_trace.rows.saturating_add(1);
            for index in 0..columns.len() {
                let value = row.get_ref(index).ok()?;
                table_trace.record_value(value);
                trace.record_value(value);
                hash.write_value_cancellable(value, cancelled)?;
            }
        }
        table_trace.completed = true;
    }
    if [
        ("event_sequence", &["aggregate_id", "seq", "owner_id"][..]),
        ("event", &["aggregate_id", "seq", "id", "type", "data"][..]),
    ]
    .iter()
    .all(|(table, columns)| {
        columns
            .iter()
            .all(|column| db_table_has_column(conn, table, column))
    }) {
        for (table, columns, order) in [
            (
                "event_sequence",
                "aggregate_id, seq, owner_id",
                "aggregate_id",
            ),
            (
                "event",
                "aggregate_id, seq, id, type, data",
                "aggregate_id, seq",
            ),
        ] {
            let mut table_trace = DbFingerprintTrace::start(table);
            hash.write_bytes(table.as_bytes());
            let mut statement = conn
                .prepare(&format!(
                    "{cluster} SELECT {columns} FROM {table}
                 WHERE aggregate_id IN (SELECT id FROM cluster) ORDER BY {order}"
                ))
                .ok()?;
            let mut query = statement.query([root_session_id]).ok()?;
            while let Some(row) = query.next().ok()? {
                if cancelled() {
                    return None;
                }
                rows = rows.saturating_add(1);
                trace.rows = rows;
                table_trace.rows = table_trace.rows.saturating_add(1);
                for index in 0..row.as_ref().column_count() {
                    let value = row.get_ref(index).ok()?;
                    table_trace.record_value(value);
                    trace.record_value(value);
                    hash.write_value_cancellable(value, cancelled)?;
                }
            }
            table_trace.completed = true;
        }
    }
    trace.completed = true;
    (rows > 0).then(|| (hash.finish(), rows))
}

const MAX_V2_FINGERPRINT_ROWS: u64 = 500_000;
const MAX_V2_FINGERPRINT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_V2_FINGERPRINT_ROW_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_V2_CLUSTER_SESSIONS: usize = 256;

fn v2_cluster_ids(conn: &Connection, root_session_id: &str) -> Option<Vec<String>> {
    let mut statement = conn
        .prepare(
            "WITH RECURSIVE cluster(id) AS (
             SELECT id FROM session_v2 WHERE id = ?1 UNION
             SELECT s.id FROM session_v2 s JOIN cluster c ON s.parent_id = c.id LIMIT ?2)
         SELECT CASE WHEN length(CAST(id AS BLOB)) <= 256 THEN id END FROM cluster ORDER BY id",
        )
        .ok()?;
    let ids = statement
        .query_map(
            params![root_session_id, (MAX_V2_CLUSTER_SESSIONS + 1) as i64],
            |row| row.get::<_, String>(0),
        )
        .ok()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .ok()?;
    (!ids.is_empty() && ids.len() <= MAX_V2_CLUSTER_SESSIONS).then_some(ids)
}

fn database_identity(conn: &Connection) -> Option<String> {
    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return Some("memory".to_owned());
    };
    database_identity_at_path(Path::new(path))
}

fn database_identity_at_path(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    crate::discovery::SourceStat::from_open_std_file(&file)?.identity
}

fn verify_path_identity(path: &Path, expected_identity: &str) -> Option<()> {
    (database_identity_at_path(path)?.as_str() == expected_identity).then_some(())
}

fn verify_database_identity(conn: &Connection, expected_identity: &str) -> Option<()> {
    sqlite_connection_matches_path(conn)?;
    (database_identity(conn)?.as_str() == expected_identity).then_some(())
}

#[cfg(unix)]
fn sqlite_connection_matches_path(conn: &Connection) -> Option<()> {
    if conn.path().is_none_or(str::is_empty) {
        return Some(());
    }
    let mut has_moved = 0_i32;
    // The connection stays borrowed and is not used concurrently during this file-control call.
    let database = unsafe { conn.handle() };
    // SQLite writes whether its open file still matches the database path.
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            database,
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_HAS_MOVED,
            (&mut has_moved as *mut i32).cast(),
        )
    };
    (result == rusqlite::ffi::SQLITE_OK && has_moved == 0).then_some(())
}

#[cfg(windows)]
fn sqlite_connection_matches_path(conn: &Connection) -> Option<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };

    let Some(path) = conn.path().filter(|path| !path.is_empty()) else {
        return Some(());
    };
    let mut sqlite_handle: windows_sys::Win32::Foundation::HANDLE = std::ptr::null_mut();
    // The connection stays borrowed and is not used concurrently during this file-control call.
    let database = unsafe { conn.handle() };
    let result = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            database,
            c"main".as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
            (&mut sqlite_handle as *mut windows_sys::Win32::Foundation::HANDLE).cast(),
        )
    };
    if result != rusqlite::ffi::SQLITE_OK || sqlite_handle.is_null() {
        return None;
    }
    let path_file = std::fs::File::open(path).ok()?;
    let mut opened = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    let mut current = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    if unsafe { GetFileInformationByHandle(sqlite_handle, opened.as_mut_ptr()) } == 0
        || unsafe { GetFileInformationByHandle(path_file.as_raw_handle(), current.as_mut_ptr()) }
            == 0
    {
        return None;
    }
    let opened = unsafe { opened.assume_init() };
    let current = unsafe { current.assume_init() };
    (opened.dwVolumeSerialNumber == current.dwVolumeSerialNumber
        && opened.nFileIndexHigh == current.nFileIndexHigh
        && opened.nFileIndexLow == current.nFileIndexLow)
        .then_some(())
}

#[cfg(not(any(unix, windows)))]
fn sqlite_connection_matches_path(conn: &Connection) -> Option<()> {
    conn.path().is_none_or(str::is_empty).then_some(())
}

fn hash_database_identity(hash: &mut DbFingerprintHash, identity: &str) {
    hash.write_bytes(identity.as_bytes());
}

fn db_session_v2_admission_revision(
    conn: &Connection,
    root_session_id: &str,
    database_identity: &str,
) -> Option<String> {
    let ids = v2_cluster_ids(conn, root_session_id)?;
    let has_event_sequence =
        crate::analysis::opencode_v2_schema::has_event_sequence_contract(conn).ok()?;
    let mut hash = DbFingerprintHash::new();
    hash_database_identity(&mut hash, database_identity);
    let mut bytes_seen = 0_usize;
    for id in ids {
        hash.write_bytes(id.as_bytes());
        for (table, predicate, columns, order, limit) in [
            (
                "session_v2",
                "id",
                &[
                    "id",
                    "parent_id",
                    "fork_session_id",
                    "directory",
                    "title",
                    "time_created",
                    "time_updated",
                    "time_archived",
                ][..],
                "id",
                1,
            ),
            (
                "session_message",
                "session_id",
                &[
                    "id",
                    "session_id",
                    "type",
                    "seq",
                    "time_created",
                    "time_updated",
                ][..],
                "seq DESC",
                8,
            ),
        ] {
            hash.write_bytes(table.as_bytes());
            let (selected, byte_length) =
                bounded_v2_columns(conn, table, columns, MAX_V2_ADMISSION_BYTES as u64);
            let mut statement = conn.prepare(&format!(
                "SELECT {selected}, {byte_length} FROM {table} WHERE {predicate} = ?1 ORDER BY {order} LIMIT {limit}"
            )).ok()?;
            let mut rows = statement.query([&id]).ok()?;
            while let Some(row) = rows.next().ok()? {
                if row.get::<_, u64>(columns.len()).ok()? > MAX_V2_ADMISSION_BYTES as u64 {
                    return None;
                }
                for index in 0..columns.len() {
                    let value = row.get_ref(index).ok()?;
                    bytes_seen = bytes_seen.saturating_add(match value {
                        ValueRef::Null => 0,
                        ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len(),
                    });
                    if bytes_seen > MAX_V2_ADMISSION_BYTES {
                        return None;
                    }
                    hash.write_value(value);
                }
            }
        }
        if has_event_sequence {
            let seq = conn
                .query_row(
                    "SELECT seq FROM event_sequence WHERE aggregate_id = ?1",
                    [&id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .ok()?;
            hash.write_value(seq.map(ValueRef::Integer).unwrap_or(ValueRef::Null));
        }
    }
    Some(format!("scope-v2:{:016x}", hash.finish()))
}

fn db_session_v2_fingerprint_cancellable_with_identity(
    conn: &Connection,
    root_session_id: &str,
    database_identity: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<(u64, u64)> {
    let ids = v2_cluster_ids(conn, root_session_id)?;
    let mut hash = DbFingerprintHash::new();
    hash_database_identity(&mut hash, database_identity);
    hash.write_bytes(root_session_id.as_bytes());
    let mut rows_seen = 0_u64;
    let mut bytes_seen = 0_u64;
    for (table, columns, join, order) in [
        (
            "session_v2",
            &[
                "id",
                "parent_id",
                "fork_session_id",
                "fork_boundary",
                "directory",
                "title",
                "version",
                "metadata",
                "cost",
                "tokens_input",
                "tokens_output",
                "tokens_reasoning",
                "tokens_cache_read",
                "tokens_cache_write",
                "revert",
                "permission",
                "agent",
                "model",
                "time_created",
                "time_updated",
                "time_archived",
            ][..],
            "session_v2.id IN (SELECT id FROM cluster)",
            "session_v2.id",
        ),
        (
            "session_message",
            &[
                "id",
                "session_id",
                "type",
                "seq",
                "time_created",
                "time_updated",
                "data",
            ][..],
            "session_message.session_id IN (SELECT id FROM cluster)",
            "session_message.session_id, session_message.seq, session_message.id",
        ),
    ] {
        hash.write_bytes(table.as_bytes());
        for column in columns {
            hash.write_bytes(column.as_bytes());
        }
        let (selected, byte_length) =
            bounded_v2_columns(conn, table, columns, MAX_V2_FINGERPRINT_ROW_BYTES);
        let placeholders = sql_in_placeholders(ids.len());
        let sql = format!(
            "WITH cluster(id) AS (SELECT id FROM session_v2 WHERE id IN ({placeholders}))
              SELECT {selected}, {byte_length} FROM {table}
               WHERE {join} ORDER BY {order}"
        );
        let mut statement = conn.prepare(&sql).ok()?;
        let mut query = statement.query(params_from_iter(&ids)).ok()?;
        while let Some(row) = query.next().ok()? {
            if cancelled() {
                return None;
            }
            rows_seen = rows_seen.saturating_add(1);
            if rows_seen > MAX_V2_FINGERPRINT_ROWS {
                return None;
            }
            if row.get::<_, u64>(columns.len()).ok()? > MAX_V2_FINGERPRINT_ROW_BYTES {
                return None;
            }
            let mut row_bytes = 0_u64;
            for index in 0..columns.len() {
                let value = row.get_ref(index).ok()?;
                let bytes = match value {
                    ValueRef::Null => 0,
                    ValueRef::Integer(_) | ValueRef::Real(_) => 8,
                    ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len() as u64,
                };
                row_bytes = row_bytes.saturating_add(bytes);
                bytes_seen = bytes_seen.saturating_add(bytes);
                if row_bytes > MAX_V2_FINGERPRINT_ROW_BYTES || bytes_seen > MAX_V2_FINGERPRINT_BYTES
                {
                    return None;
                }
                hash.write_value_cancellable(value, cancelled)?;
            }
            if table == "session_v2" {
                let (dependency_rows, dependency_bytes) =
                    hash_v2_fork_boundary_dependency(conn, row, &mut hash, cancelled)?;
                rows_seen = rows_seen.saturating_add(dependency_rows);
                bytes_seen = bytes_seen.saturating_add(dependency_bytes);
                if rows_seen > MAX_V2_FINGERPRINT_ROWS || bytes_seen > MAX_V2_FINGERPRINT_BYTES {
                    return None;
                }
            }
        }
    }
    (rows_seen > 0).then(|| (hash.finish(), rows_seen))
}

fn bounded_v2_columns(
    conn: &Connection,
    table: &str,
    columns: &[&str],
    max_bytes: u64,
) -> (String, String) {
    let mut values = Vec::new();
    let mut sizes = Vec::new();
    for column in columns {
        if !db_table_has_column(conn, table, column) {
            values.push("NULL".to_owned());
            continue;
        }
        let value = format!("{table}.{column}");
        let size = format!(
            "CASE typeof({value}) WHEN 'null' THEN 0 WHEN 'integer' THEN 8 WHEN 'real' THEN 8 ELSE length(CAST({value} AS BLOB)) END"
        );
        values.push(value);
        sizes.push(format!("({size})"));
    }
    let byte_length = if sizes.is_empty() {
        "0".into()
    } else {
        sizes.join(" + ")
    };
    let selected = values
        .into_iter()
        .map(|value| format!("CASE WHEN ({byte_length}) <= {max_bytes} THEN {value} END"))
        .collect::<Vec<_>>()
        .join(", ");
    (selected, byte_length)
}

fn hash_v2_fork_boundary_dependency(
    conn: &Connection,
    row: &rusqlite::Row<'_>,
    hash: &mut DbFingerprintHash,
    cancelled: &dyn Fn() -> bool,
) -> Option<(u64, u64)> {
    if cancelled() {
        return None;
    }
    hash.write_bytes(b"fork-parent-boundary");
    let (ValueRef::Text(parent), ValueRef::Text(boundary)) =
        (row.get_ref(2).ok()?, row.get_ref(3).ok()?)
    else {
        hash.write_bytes(b"unresolved");
        return Some((0, 0));
    };
    // Match the parser's byte cap and duplicate-key rules before the lookup.
    if parent.len() > 4096 || boundary.len() > 4096 {
        hash.write_bytes(b"unresolved");
        return Some((0, 0));
    }
    let parent = std::str::from_utf8(parent).ok()?;
    let boundary = serde_json::from_slice::<Value>(boundary).ok();
    let message_id = boundary.as_ref().and_then(|boundary| {
        matches!(
            boundary.get("type").and_then(Value::as_str),
            Some("before" | "through")
        )
        .then(|| boundary.get("messageID").and_then(Value::as_str))
        .flatten()
    });
    let Some(message_id) = message_id else {
        hash.write_bytes(b"unresolved");
        return Some((0, 0));
    };
    let sequence = conn
        .query_row(
            "SELECT seq FROM session_message WHERE session_id = ?1 AND id = ?2",
            params![parent, message_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .ok()?;
    if cancelled() {
        return None;
    }
    match sequence {
        Some(sequence) => {
            hash.write_bytes(b"present");
            hash.write_value(ValueRef::Integer(sequence));
            Some((1, 8))
        }
        None => {
            hash.write_bytes(b"missing");
            Some((1, 0))
        }
    }
}

struct DbFingerprintTrace {
    table: &'static str,
    started: Instant,
    rows: u64,
    bytes: u64,
    completed: bool,
}

impl DbFingerprintTrace {
    fn start(table: &'static str) -> Self {
        let trace = Self {
            table,
            started: Instant::now(),
            rows: 0,
            bytes: 0,
            completed: false,
        };
        tracing::debug!(
            event = "opencode_db_fingerprint_started",
            table,
            "OpenCode strict fingerprint stage started"
        );
        trace
    }

    fn record_value(&mut self, value: ValueRef<'_>) {
        let bytes = match value {
            ValueRef::Null => 0,
            ValueRef::Integer(_) | ValueRef::Real(_) => 8,
            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes.len() as u64,
        };
        self.bytes = self.bytes.saturating_add(bytes);
    }
}

impl Drop for DbFingerprintTrace {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        tracing::debug!(
            event = "opencode_db_fingerprint_finished",
            table = self.table,
            rows = self.rows,
            value_bytes = self.bytes,
            duration_ms = elapsed.as_millis() as u64,
            completed = self.completed,
            "OpenCode strict fingerprint stage finished"
        );
        if elapsed >= Duration::from_secs(1) {
            tracing::warn!(
                event = "opencode_db_fingerprint_slow",
                table = self.table,
                rows = self.rows,
                value_bytes = self.bytes,
                duration_ms = elapsed.as_millis() as u64,
                completed = self.completed,
                "OpenCode strict fingerprint stage took at least one second"
            );
        }
    }
}

fn db_table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, column],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

struct DbFingerprintHash(u64);

impl DbFingerprintHash {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET_BASIS)
    }

    fn write_value(&mut self, value: ValueRef<'_>) {
        let _ = self.write_value_cancellable(value, &|| false);
    }

    fn write_value_cancellable(
        &mut self,
        value: ValueRef<'_>,
        cancelled: &dyn Fn() -> bool,
    ) -> Option<()> {
        match value {
            ValueRef::Null => self.write_bytes(&[0]),
            ValueRef::Integer(value) => {
                self.write_bytes(&[1]);
                self.write_bytes(&value.to_le_bytes());
            }
            ValueRef::Real(value) => {
                self.write_bytes(&[2]);
                self.write_bytes(&value.to_bits().to_le_bytes());
            }
            ValueRef::Text(value) => {
                self.write_bytes(&[3]);
                self.write_bytes_cancellable(value, cancelled)?;
            }
            ValueRef::Blob(value) => {
                self.write_bytes(&[4]);
                self.write_bytes_cancellable(value, cancelled)?;
            }
        }
        Some(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        let _ = self.write_bytes_cancellable(bytes, &|| false);
    }

    fn write_bytes_cancellable(
        &mut self,
        bytes: &[u8],
        cancelled: &dyn Fn() -> bool,
    ) -> Option<()> {
        for byte in (bytes.len() as u64).to_le_bytes().iter() {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(Self::PRIME);
        }
        for chunk in bytes.chunks(64 * 1024) {
            if cancelled() {
                return None;
            }
            for byte in chunk {
                self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(Self::PRIME);
            }
        }
        Some(())
    }

    fn finish(self) -> u64 {
        self.0
    }
}

pub(crate) fn db_session_has_parent_id(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session') WHERE name = 'parent_id')",
        [],
        |row| row.get(0),
    )
    .unwrap_or(false)
}

fn recent_db_session_ids(conn: &Connection, cutoff: i64) -> HashSet<String> {
    let mut ids = HashSet::new();

    for query in [
        "SELECT id FROM session WHERE COALESCE(time_updated, time_created) >= ?1",
        "SELECT DISTINCT session_id FROM message WHERE COALESCE(time_updated, time_created) >= ?1",
        "SELECT DISTINCT session_id FROM part WHERE COALESCE(time_updated, time_created) >= ?1",
    ] {
        let Ok(mut stmt) = conn.prepare(query) else {
            continue;
        };
        let Ok(rows) = stmt.query_map(params![cutoff.saturating_mul(1000)], |row| {
            row.get::<_, String>(0)
        }) else {
            continue;
        };
        for id in rows.flatten() {
            ids.insert(id);
        }
    }

    ids
}

fn db_session_ids_with_ancestors(
    conn: &Connection,
    session_ids: &HashSet<String>,
) -> HashSet<String> {
    let mut all_ids = session_ids.clone();
    let mut frontier = session_ids.clone();

    while !frontier.is_empty() {
        let placeholders = sql_in_placeholders(frontier.len());
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT id, parent_id FROM session WHERE id IN ({placeholders})"
        )) else {
            break;
        };

        let Ok(rows) = stmt.query_map(params_from_iter(frontier.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
            ))
        }) else {
            break;
        };

        let mut next_frontier = HashSet::new();
        for (_, parent_id) in rows.flatten() {
            if let Some(parent_id) = parent_id
                && all_ids.insert(parent_id.clone())
            {
                next_frontier.insert(parent_id);
            }
        }
        frontier = next_frontier;
    }

    all_ids
}

fn db_session_ids_for_root(conn: &Connection, root_session_id: &str) -> HashSet<String> {
    let Some(cluster) = db_v1_cluster_cte(conn, None) else {
        return HashSet::new();
    };
    let Ok(mut statement) = conn.prepare(&format!("{cluster} SELECT id FROM cluster")) else {
        return HashSet::new();
    };
    let Ok(rows) = statement.query_map([root_session_id], |row| row.get::<_, String>(0)) else {
        return HashSet::new();
    };
    rows.collect::<rusqlite::Result<_>>().unwrap_or_default()
}

fn db_v1_cluster_cte(conn: &Connection, limit: Option<usize>) -> Option<String> {
    let schema = opencode_sqlite_schema(conn).ok()?;
    if !schema.has_v1() {
        return None;
    }
    let exclusion = if schema.has_v2() {
        " AND NOT EXISTS(SELECT 1 FROM session_v2 WHERE session_v2.id = session.id)"
    } else {
        ""
    };
    let descendants = if db_session_has_parent_id(conn) {
        format!(
            "UNION SELECT session.id FROM session JOIN cluster ON session.parent_id = cluster.id WHERE 1 {exclusion}"
        )
    } else {
        String::new()
    };
    let limit = limit
        .map(|limit| format!("LIMIT {limit}"))
        .unwrap_or_default();
    Some(format!(
        "WITH RECURSIVE cluster(id) AS (SELECT session.id FROM session WHERE session.id = ?1 {exclusion} {descendants} {limit})"
    ))
}

fn fetch_db_sessions(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<(String, SessionRecord)> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, parent_id, directory, title, time_created, time_updated, data FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
                row.get::<_, String>(2).ok(),
                row.get::<_, String>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, i64>(5).ok(),
                row.get::<_, String>(6).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, parent_session_id, directory, title, time_created, time_updated, raw_json)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json,
                        },
                    )
                },
            )
            .collect();
    }

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, directory, title, time_created, time_updated, data FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1).ok(),
                row.get::<_, String>(2).ok(),
                row.get::<_, i64>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, String>(5).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, directory, title, time_created, time_updated, raw_json)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id: None,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json,
                        },
                    )
                },
            )
            .collect();
    }

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, parent_id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
                row.get::<_, String>(2).ok(),
                row.get::<_, String>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, i64>(5).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, parent_session_id, directory, title, time_created, time_updated)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json: None,
                        },
                    )
                },
            )
            .collect();
    }

    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1).ok(),
            row.get::<_, String>(2).ok(),
            row.get::<_, i64>(3).ok(),
            row.get::<_, i64>(4).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(
            |(session_id, directory, title, time_created, time_updated)| {
                build_db_session_record(
                    db_path,
                    DbSessionRowData {
                        session_id,
                        parent_session_id: None,
                        directory,
                        title,
                        time_created,
                        time_updated,
                        raw_json: None,
                    },
                )
            },
        )
        .collect()
}

fn fetch_db_sessions_light(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<(String, SessionRecord)> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());

    if let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, parent_id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) {
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1).ok().flatten(),
                row.get::<_, String>(2).ok(),
                row.get::<_, String>(3).ok(),
                row.get::<_, i64>(4).ok(),
                row.get::<_, i64>(5).ok(),
            ))
        }) else {
            return Vec::new();
        };

        return rows
            .flatten()
            .map(
                |(session_id, parent_session_id, directory, title, time_created, time_updated)| {
                    build_db_session_record(
                        db_path,
                        DbSessionRowData {
                            session_id,
                            parent_session_id,
                            directory,
                            title,
                            time_created,
                            time_updated,
                            raw_json: None,
                        },
                    )
                },
            )
            .collect();
    }

    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, directory, title, time_created, time_updated FROM session WHERE id IN ({placeholders})"
    )) else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1).ok(),
            row.get::<_, String>(2).ok(),
            row.get::<_, i64>(3).ok(),
            row.get::<_, i64>(4).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(
            |(session_id, directory, title, time_created, time_updated)| {
                build_db_session_record(
                    db_path,
                    DbSessionRowData {
                        session_id,
                        parent_session_id: None,
                        directory,
                        title,
                        time_created,
                        time_updated,
                        raw_json: None,
                    },
                )
            },
        )
        .collect()
}

pub(super) fn fetch_db_activity_by_session(
    conn: &Connection,
    session_ids: &HashSet<String>,
) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    if session_ids.is_empty() {
        return out;
    }
    let placeholders = sql_in_placeholders(session_ids.len());
    for table in ["message", "part"] {
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT session_id, time_created, time_updated FROM {table} WHERE session_id IN ({placeholders})"
        )) else {
            continue;
        };
        let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1).ok(),
                row.get::<_, i64>(2).ok(),
            ))
        }) else {
            continue;
        };
        for (session_id, time_created, time_updated) in rows.flatten() {
            if let Some(ts) = normalize_db_epoch(time_created).max(normalize_db_epoch(time_updated))
            {
                out.entry(session_id)
                    .and_modify(|current| *current = (*current).max(ts))
                    .or_insert(ts);
            }
        }
    }
    out
}

fn query_db_session_metadata_row(
    conn: &Connection,
    session_id: &str,
) -> Option<DbSessionMetadataRow> {
    if let Ok(mut stmt) =
        conn.prepare("SELECT id, directory, title FROM session WHERE id = ?1 LIMIT 1")
    {
        return stmt
            .query_row(params![session_id], |row| {
                Ok(DbSessionMetadataRow {
                    session_id: row.get::<_, String>(0)?,
                    directory: row.get::<_, String>(1).ok(),
                    title: row.get::<_, String>(2).ok().and_then(|t| {
                        let trimmed = t.trim().to_string();
                        (!trimmed.is_empty()).then_some(trimmed)
                    }),
                })
            })
            .ok();
    }
    None
}

fn build_db_session_record(db_path: &Path, row: DbSessionRowData) -> (String, SessionRecord) {
    let DbSessionRowData {
        session_id,
        parent_session_id,
        directory,
        title,
        time_created,
        time_updated,
        raw_json,
    } = row;

    let raw = raw_json
        .as_deref()
        .and_then(parse_json)
        .unwrap_or_else(|| json!({}));
    let raw = merge_json_object(
        raw,
        json!({
            "id": session_id.clone(),
            "parentID": parent_session_id.clone(),
            "directory": directory.clone(),
            "title": title.clone(),
            "time": {
                "created": normalize_db_epoch(time_created),
                "updated": normalize_db_epoch(time_updated),
            }
        }),
    );
    let record = SessionRecord {
        directory,
        title,
        parent_session_id,
        source_file: format!("{}#session/{}", db_path.display(), session_id),
        created_at: normalize_db_epoch(time_created),
        updated_at: normalize_db_epoch(time_updated),
        file_mtime: None,
        raw,
    };
    (session_id, record)
}

fn fetch_db_messages(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<MessageRecord> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, session_id, time_created, time_updated, data FROM message WHERE session_id IN ({placeholders})"
    ))
    else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2).ok(),
            row.get::<_, i64>(3).ok(),
            row.get::<_, String>(4).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(|(id, session_id, time_created, time_updated, raw_json)| {
            let raw = raw_json
                .as_deref()
                .and_then(parse_json)
                .unwrap_or_else(|| json!({}));
            let raw = merge_json_object(
                raw,
                json!({
                    "id": id,
                    "sessionID": session_id,
                    "time": {
                        "created": normalize_db_epoch(time_created),
                        "updated": normalize_db_epoch(time_updated),
                    }
                }),
            );

            MessageRecord {
                id: id.clone(),
                session_id: session_id.clone(),
                role: raw.get("role").and_then(Value::as_str).map(str::to_string),
                source_file: format!("{}#message/{}", db_path.display(), id),
                created_at: raw
                    .pointer("/time/created")
                    .and_then(parse_epoch_from_json_value)
                    .or_else(|| normalize_db_epoch(time_created))
                    .or_else(|| normalize_db_epoch(time_updated)),
                file_mtime: None,
                raw,
            }
        })
        .collect()
}

fn fetch_db_parts(
    conn: &Connection,
    db_path: &Path,
    session_ids: &HashSet<String>,
) -> Vec<PartRecord> {
    if session_ids.is_empty() {
        return Vec::new();
    }

    let placeholders = sql_in_placeholders(session_ids.len());
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT id, message_id, session_id, time_created, time_updated, data FROM part WHERE session_id IN ({placeholders})"
    ))
    else {
        return Vec::new();
    };

    let Ok(rows) = stmt.query_map(params_from_iter(session_ids.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3).ok(),
            row.get::<_, i64>(4).ok(),
            row.get::<_, String>(5).ok(),
        ))
    }) else {
        return Vec::new();
    };

    rows.flatten()
        .map(
            |(id, message_id, session_id, time_created, time_updated, raw_json)| {
                let raw = raw_json
                    .as_deref()
                    .and_then(parse_json)
                    .unwrap_or_else(|| json!({}));
                let raw = merge_json_object(
                    raw,
                    json!({
                        "id": id,
                        "messageID": message_id,
                        "sessionID": session_id,
                        "time": {
                            "created": normalize_db_epoch(time_created),
                            "updated": normalize_db_epoch(time_updated),
                        }
                    }),
                );

                PartRecord {
                    id: id.clone(),
                    message_id: Some(message_id),
                    session_id: Some(session_id),
                    part_type: raw.get("type").and_then(Value::as_str).map(str::to_string),
                    source_file: format!("{}#part/{}", db_path.display(), id),
                    created_at: raw
                        .pointer("/time/created")
                        .and_then(parse_epoch_from_json_value)
                        .or_else(|| normalize_db_epoch(time_created))
                        .or_else(|| normalize_db_epoch(time_updated)),
                    file_mtime: None,
                    raw,
                }
            },
        )
        .collect()
}
