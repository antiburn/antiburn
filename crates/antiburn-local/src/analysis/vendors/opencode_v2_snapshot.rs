//! Reads the pinned OpenCode V2 SQLite message projection.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;

use crate::analysis::SourceFormat;
use crate::analysis::opencode_v2_schema::OpenCodeV2SnapshotError;

const MAX_SNAPSHOT_ROW_BYTES: i64 = 8 * 1024 * 1024;
pub(super) const MAX_SNAPSHOT_ROWS: u64 = 100_000;
pub(super) const MAX_SNAPSHOT_TOTAL_BYTES: u64 = 128 * 1024 * 1024;

/// One source-shaped message reconstructed from the V2 projection row.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotMessage {
    pub session_id: String,
    pub seq: i64,
    pub value: Value,
    pub session: Arc<SnapshotSession>,
}

/// Native session facts from the same transaction as its messages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapshotSession {
    pub transfer: bool,
    pub fork_session_id: Option<String>,
    pub fork_boundary: Option<Value>,
    pub inherited_through: Option<i64>,
    pub unresolved_fork: bool,
    pub tokens: Option<Value>,
    /// OpenCode calculates this value. It is not a billing receipt.
    pub calculated_cost: Option<f64>,
    pub accounting_malformed: bool,
    /// Only a complete, single-model child history supports attribution here.
    pub child_models: HashMap<String, SnapshotChildModel>,
}

impl SnapshotSession {
    pub fn source_format(&self) -> SourceFormat {
        if self.transfer {
            SourceFormat::OpenCodeV2SessionExport
        } else {
            SourceFormat::OpenCodeSqliteV2
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SnapshotChildModel {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub first_step_ms: Option<i64>,
}

/// Per-session latest durable sequence from one SQLite read snapshot.
pub type CaptureBaseline = HashMap<String, Option<u64>>;

/// Counts the rows read from one complete SQLite cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotReadSummary {
    pub rows_seen: u64,
    pub oversized_rows: u64,
    pub malformed_rows: u64,
    pub capacity_limit_reached: bool,
    pub bytes_visited: u64,
    pub session: Arc<SnapshotSession>,
}

impl SnapshotReadSummary {
    pub fn is_partial(&self) -> bool {
        self.oversized_rows > 0 || self.malformed_rows > 0 || self.capacity_limit_reached
    }
}

/// Stream one session in the caller's read-only transaction, with shared cluster limits.
pub(super) fn visit_snapshot_connection(
    connection: &Connection,
    session_id: &str,
    cancel: &dyn Fn() -> bool,
    visitor: &mut dyn FnMut(SnapshotMessage) -> anyhow::Result<()>,
    row_limit: u64,
    byte_limit: u64,
) -> anyhow::Result<SnapshotReadSummary> {
    validate_schema(connection)?;

    let session_exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_v2 WHERE id = ?1)",
        [session_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !session_exists {
        return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
    }
    let session = Arc::new(read_session(connection, session_id)?);

    let mut statement = connection.prepare(
        "SELECT
             CASE WHEN row_bytes <= ?2 THEN id END,
             CASE WHEN row_bytes <= ?2 THEN session_id END,
             CASE WHEN row_bytes <= ?2 THEN type END,
             seq,
             CASE WHEN row_bytes <= ?2 THEN data END,
             row_bytes
           FROM (
             SELECT id, session_id, type, seq, data,
                    length(CAST(id AS BLOB))
                      + length(CAST(session_id AS BLOB))
                      + length(CAST(type AS BLOB))
                      + length(CAST(data AS BLOB)) AS row_bytes
               FROM session_message
              WHERE session_id = ?1
           )
          ORDER BY session_id, seq",
    )?;
    let mut rows = statement.query(params![session_id, MAX_SNAPSHOT_ROW_BYTES])?;
    let mut summary = SnapshotReadSummary {
        rows_seen: 0,
        oversized_rows: 0,
        malformed_rows: 0,
        capacity_limit_reached: false,
        bytes_visited: 0,
        session: Arc::clone(&session),
    };
    let mut previous_seq = None;
    while let Some(row) = rows.next()? {
        if summary.rows_seen >= row_limit {
            summary.capacity_limit_reached = true;
            break;
        }
        if cancel() {
            return Err(OpenCodeV2SnapshotError::Cancelled.into());
        }
        summary.rows_seen = summary.rows_seen.saturating_add(1);
        let row_bytes = row
            .get::<_, Option<i64>>(5)?
            .context("invalid message row size")?;
        if row_bytes > MAX_SNAPSHOT_ROW_BYTES {
            summary.oversized_rows = summary.oversized_rows.saturating_add(1);
            continue;
        }
        let row_bytes = row_bytes as u64;
        if summary.bytes_visited.saturating_add(row_bytes) > byte_limit {
            summary.capacity_limit_reached = true;
            break;
        }
        summary.bytes_visited = summary.bytes_visited.saturating_add(row_bytes);

        let id = required_text(row, 0, "message id")?;
        let row_session_id = required_text(row, 1, "message session id")?;
        let message_type = required_text(row, 2, "message type")?;
        let seq = row.get::<_, i64>(3).context("invalid message sequence")?;
        let data = required_text(row, 4, "message data")?;

        if row_session_id != session_id {
            return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
        }
        if previous_seq.is_some_and(|previous| seq <= previous) {
            return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
        }
        previous_seq = Some(seq);
        if seq < 0 {
            summary.malformed_rows += 1;
            continue;
        }
        let Ok(mut value) = serde_json::from_str::<Value>(&data) else {
            summary.malformed_rows += 1;
            continue;
        };
        let Some(object) = value.as_object_mut() else {
            summary.malformed_rows += 1;
            continue;
        };
        object.insert("id".into(), Value::String(id));
        object.insert("type".into(), Value::String(message_type));
        visitor(SnapshotMessage {
            session_id: row_session_id,
            seq,
            value,
            session: Arc::clone(&session),
        })?;
    }

    Ok(summary)
}

/// Read the bounded parent/descendant cluster without following fork links.
pub(super) fn session_cluster_ids(
    connection: &Connection,
    session_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "WITH RECURSIVE cluster(id) AS (
             SELECT id FROM session_v2 WHERE id = ?1 UNION
             SELECT s.id FROM session_v2 s JOIN cluster c ON s.parent_id = c.id LIMIT 257)
         SELECT CASE WHEN length(CAST(id AS BLOB)) <= 256 THEN id END FROM cluster ORDER BY id <> ?1, id",
    )?;
    let ids = statement
        .query_map([session_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.is_empty() {
        return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
    }
    if ids.len() > 256 {
        return Err(OpenCodeV2SnapshotError::SourceLimit.into());
    }
    Ok(ids)
}

/// Bind live watermarks to current native CWD and main-session ownership.
pub fn read_capture_baseline_for_scope(
    path: &Path,
    source_format: SourceFormat,
    expected_scopes: &[(String, String)],
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<CaptureBaseline> {
    if expected_scopes.iter().any(|(_, directory)| {
        directory.is_empty() || directory.len() > 4096 || directory.contains('\0')
    }) {
        return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
    }
    let session_ids = expected_scopes
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    if source_format != SourceFormat::OpenCodeSqliteV2 {
        return Err(OpenCodeV2SnapshotError::UnsupportedSource.into());
    }
    let max_sessions = crate::analysis::opencode_v2_capture::MAX_CAPTURE_SESSIONS;
    let unique_ids: HashSet<&String> = session_ids.iter().collect();
    if session_ids.len() > max_sessions {
        return Err(OpenCodeV2SnapshotError::SourceLimit.into());
    }
    if unique_ids.len() != session_ids.len()
        || session_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 1024 || id.contains('\0'))
    {
        return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
    }
    if cancel() {
        return Err(OpenCodeV2SnapshotError::Cancelled.into());
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("opening OpenCode V2 baseline")?;
    connection.execute_batch("BEGIN")?;
    validate_schema(&connection)?;
    if !crate::analysis::opencode_v2_schema::has_event_sequence_contract(&connection)? {
        return Err(OpenCodeV2SnapshotError::UnsupportedSchema.into());
    }
    {
        if !crate::analysis::opencode_v2_schema::has_fork_lineage_contract(&connection)? {
            return Err(OpenCodeV2SnapshotError::UnsupportedSchema.into());
        }
        // Missing lineage columns cannot prove main-session admission.
        let mut statement = connection.prepare(
            "SELECT CASE WHEN length(CAST(directory AS BLOB)) <= 4096 THEN directory END,\
                    CASE WHEN parent_id IS NULL THEN NULL ELSE 1 END,\
                    CASE WHEN fork_session_id IS NULL THEN NULL ELSE 1 END \
               FROM session_v2 WHERE id = ?1",
        )?;
        for (id, expected_directory) in expected_scopes {
            if cancel() {
                return Err(OpenCodeV2SnapshotError::Cancelled.into());
            }
            let native = statement
                .query_row([id], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                })
                .optional()?;
            if !native.is_some_and(|(directory, parent, fork)| {
                directory.as_deref() == Some(expected_directory.as_str())
                    && parent.is_none()
                    && fork.is_none()
            }) {
                return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
            }
        }
    }

    let mut baseline = CaptureBaseline::new();
    if !session_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", session_ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let query = format!(
            "SELECT s.id, e.seq
               FROM session_v2 s
               LEFT JOIN event_sequence e ON e.aggregate_id = s.id
              WHERE s.id IN ({placeholders})
              ORDER BY s.id"
        );
        let mut statement = connection.prepare(&query)?;
        let rows = statement.query_map(rusqlite::params_from_iter(&session_ids), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })?;
        for row in rows {
            if cancel() {
                return Err(OpenCodeV2SnapshotError::Cancelled.into());
            }
            let (session_id, sequence) = row?;
            let sequence = sequence
                .map(|sequence| {
                    u64::try_from(sequence).map_err(|_| OpenCodeV2SnapshotError::ScopeChanged)
                })
                .transpose()?;
            baseline.insert(session_id, sequence);
        }
        drop(statement);
    }
    if baseline.len() != session_ids.len() {
        return Err(OpenCodeV2SnapshotError::ScopeChanged.into());
    }
    if cancel() {
        return Err(OpenCodeV2SnapshotError::Cancelled.into());
    }
    connection.execute_batch("COMMIT")?;
    Ok(baseline)
}

fn validate_schema(connection: &Connection) -> anyhow::Result<()> {
    if !crate::analysis::opencode_v2_schema::is_supported(connection)? {
        return Err(OpenCodeV2SnapshotError::UnsupportedSchema.into());
    }
    Ok(())
}

fn required_text(row: &rusqlite::Row<'_>, index: usize, label: &str) -> anyhow::Result<String> {
    row.get::<_, String>(index)
        .with_context(|| format!("OpenCode V2 snapshot has invalid {label}"))
}

fn read_session(connection: &Connection, session_id: &str) -> anyhow::Result<SnapshotSession> {
    let names = crate::analysis::opencode_v2_schema::table_columns(connection, "session_v2")?;
    let optional = |name: &str| {
        if names.contains(name) {
            format!("CASE WHEN length(CAST({name} AS BLOB)) <= 4096 THEN {name} END")
        } else {
            "NULL".to_owned()
        }
    };
    let token_columns = [
        "tokens_input",
        "tokens_output",
        "tokens_reasoning",
        "tokens_cache_read",
        "tokens_cache_write",
    ];
    let tokens = if token_columns.iter().all(|name| names.contains(*name)) {
        let valid = token_columns
            .iter()
            .map(|column| format!("typeof({column}) IN ('integer','real')"))
            .collect::<Vec<_>>()
            .join(" AND ");
        format!(
            "CASE WHEN {valid} THEN json_object('input',tokens_input,'output',tokens_output,'reasoning',tokens_reasoning,\
         'cache',json_object('read',tokens_cache_read,'write',tokens_cache_write)) ELSE 'null' END"
        )
    } else {
        "NULL".to_owned()
    };
    let fork_present = if names.contains("fork_session_id") {
        "fork_session_id IS NOT NULL"
    } else {
        "0"
    };
    let cost = if names.contains("cost") {
        "CASE WHEN typeof(cost) IN ('integer','real') THEN cost END"
    } else {
        "NULL"
    };
    let invalid_cost = if names.contains("cost") {
        "cost IS NOT NULL AND typeof(cost) NOT IN ('integer','real')"
    } else {
        "0"
    };
    let query = format!(
        "SELECT {}, {}, {tokens}, {cost}, {fork_present}, {invalid_cost} FROM session_v2 WHERE id = ?1",
        optional("fork_session_id"),
        optional("fork_boundary"),
    );
    let (fork_session_id, boundary, tokens, calculated_cost, fork_present, accounting_malformed) =
        connection.query_row(&query, [session_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
            ))
        })?;
    let fork_boundary = boundary
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok());
    let mut session = SnapshotSession {
        fork_session_id,
        fork_boundary,
        tokens: tokens
            .as_deref()
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null)),
        calculated_cost,
        accounting_malformed,
        unresolved_fork: fork_present,
        ..SnapshotSession::default()
    };
    if let Some(parent) = &session.fork_session_id
        && let Some(boundary) = &session.fork_boundary
        && let Some(id) = boundary.get("messageID").and_then(Value::as_str)
        && let Some(kind @ ("before" | "through")) = boundary.get("type").and_then(Value::as_str)
    {
        session.inherited_through = connection
            .query_row(
                "SELECT seq FROM session_message WHERE session_id = ?1 AND id = ?2",
                params![parent, id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .and_then(|seq| {
                if kind == "before" {
                    seq.checked_sub(1)
                } else {
                    Some(seq)
                }
            });
        session.unresolved_fork = session.inherited_through.is_none();
    }
    let mut children = connection.prepare(
        "SELECT CASE WHEN length(CAST(id AS BLOB)) <= 256 THEN id END FROM session_v2 WHERE parent_id = ?1 ORDER BY id LIMIT 65",
    )?;
    let child_ids = children
        .query_map([session_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for child in child_ids.into_iter().take(64) {
        let mut messages = connection.prepare(
            "SELECT CASE WHEN length(CAST(data AS BLOB)) <= 8192 AND json_valid(data)\
                    THEN CASE WHEN json_type(data, '$.model.id') = 'text'\
                              THEN json_extract(data, '$.model.id') END END,\
                    CASE WHEN length(CAST(data AS BLOB)) <= 8192 AND json_valid(data)\
                    THEN CASE WHEN json_type(data, '$.model.providerID') = 'text'\
                              THEN json_extract(data, '$.model.providerID') END END,\
                    CASE WHEN length(CAST(data AS BLOB)) <= 8192 AND json_valid(data)\
                    THEN CASE WHEN json_type(data, '$.time.created') = 'integer'\
                                   AND typeof(json_extract(data, '$.time.created')) = 'integer'\
                              THEN json_extract(data, '$.time.created') END END \
               FROM session_message WHERE session_id = ?1 AND type = 'assistant' ORDER BY seq LIMIT 33",
        )?;
        let models = messages
            .query_map([&child], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let model = models
            .first()
            .and_then(|(model, _, _)| model.as_deref())
            .filter(|model| {
                !model.is_empty()
                    && model.len() <= 256
                    && models.len() <= 32
                    && models.iter().all(|(other, provider, created)| {
                        other.as_deref() == Some(*model)
                            && provider.as_deref().is_some_and(|value| !value.is_empty())
                            && created.is_some()
                    })
            })
            .map(str::to_owned);
        let provider = models
            .first()
            .and_then(|(_, provider, _)| provider.as_deref())
            .filter(|provider| {
                provider.len() <= 256
                    && models
                        .iter()
                        .all(|(_, other, _)| other.as_deref() == Some(*provider))
            })
            .map(str::to_owned);
        let first_step_ms = models.first().and_then(|(_, _, created)| *created);
        session.child_models.insert(
            child,
            SnapshotChildModel {
                model,
                provider,
                first_step_ms,
            },
        );
    }
    Ok(session)
}
