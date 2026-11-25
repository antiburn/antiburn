//! Bounded reads of the Cursor tables that carry ownership evidence.

use crate::opencode::sql_error;
use crate::{Error, Result};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) const PROOF_BYTES: u64 = 1024 * 1024;

/// Check the optional native composer index before reading workspace evidence.
pub(super) fn composer_index(db: &Connection) -> Result<bool> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_list('composerHeaders')
                           WHERE schema = 'main' AND name = 'composerHeaders' COLLATE BINARY)",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if exists {
        rowid(db, "composerHeaders", "composerId", "workspaceId")?;
    }
    Ok(exists)
}

/// Read the exact composer's bounded workspace ID, never its index payload.
pub(super) fn indexed_workspace(db: &Connection, composer: &str) -> Result<Option<String>> {
    match workspace_index_proof(db, composer)? {
        WorkspaceProof::Unambiguous(workspace) => Ok(workspace),
        WorkspaceProof::Ambiguous => Err(Error::UnsupportedSource),
    }
}

/// Separate duplicate composer records from errors that affect the full query.
pub(super) enum WorkspaceProof {
    Unambiguous(Option<String>),
    Ambiguous,
}

/// Read at most two index records to detect duplicate proof for one composer.
pub(super) fn workspace_index_proof(db: &Connection, composer: &str) -> Result<WorkspaceProof> {
    let mut query = db
        .prepare(
            "SELECT CASE WHEN typeof(workspaceId) = 'text' AND octet_length(workspaceId) <= 128
                        THEN workspaceId END
             FROM main.composerHeaders
             WHERE typeof(composerId) = 'text' AND composerId = ?1 COLLATE BINARY LIMIT 2",
        )
        .map_err(sql_error)?;
    let mut rows = query.query([composer]).map_err(sql_error)?;
    let Some(row) = rows.next().map_err(sql_error)? else {
        return Ok(WorkspaceProof::Unambiguous(None));
    };
    let workspace: Option<String> = row.get(0).map_err(sql_error)?;
    if rows.next().map_err(sql_error)?.is_some() {
        return Ok(WorkspaceProof::Ambiguous);
    }
    Ok(WorkspaceProof::Unambiguous(
        workspace.filter(|id| crate::contract::safe_id(id)),
    ))
}

/// Check table layout before reading keys or values. Return a safe row locator.
pub(super) fn rowid(db: &Connection, table: &str, key: &str, value: &str) -> Result<&'static str> {
    let layout = db
        .query_row(
            "SELECT type, wr, ncol FROM pragma_table_list(?1)
             WHERE schema = 'main' AND name = ?1 COLLATE BINARY",
            [table],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error)?;
    if !matches!(layout, Some((kind, 0, 2..=64)) if kind == "table") {
        return Err(Error::UnsupportedSource);
    }
    let mut statement = db
        .prepare("SELECT name, hidden FROM pragma_table_xinfo(?1, 'main')")
        .map_err(sql_error)?;
    let mut rows = statement.query([table]).map_err(sql_error)?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let name = row.get_ref(0).map_err(sql_error)?;
        let name = name.as_str().map_err(|_| Error::UnsupportedSource)?;
        if row.get::<_, i64>(1).map_err(sql_error)? != 0 || name.len() > 128 {
            return Err(Error::UnsupportedSource);
        }
        columns.push(name.to_owned());
    }
    if !columns.iter().any(|column| column == key) || !columns.iter().any(|column| column == value)
    {
        return Err(Error::UnsupportedSource);
    }
    ["_rowid_", "rowid", "oid"]
        .into_iter()
        .find(|alias| {
            !columns
                .iter()
                .any(|column| column.eq_ignore_ascii_case(alias))
        })
        .ok_or(Error::UnsupportedSource)
}

/// Look up one exact text key. Duplicate rows cannot prove ownership.
pub(super) fn exact_row(db: &Connection, table: &str, rowid: &str, key: &str) -> Result<i64> {
    let mut statement = db
        .prepare(&format!(
            "SELECT {rowid} FROM main.{table}
             WHERE typeof(key) = 'text' AND key = ?1 COLLATE BINARY LIMIT 2"
        ))
        .map_err(sql_error)?;
    let mut rows = statement.query([key]).map_err(sql_error)?;
    let result = rows
        .next()
        .map_err(sql_error)?
        .ok_or(Error::SourceChanged)?
        .get(0)
        .map_err(sql_error)?;
    if rows.next().map_err(sql_error)?.is_some() {
        return Err(Error::UnsupportedSource);
    }
    Ok(result)
}

/// Read only a bounded native JSON proof value. SQLite cells keep their raw
/// storage type and bytes when the writer exports them.
pub(super) fn proof_bytes(
    db: &Connection,
    table: &str,
    rowid: &str,
    row: i64,
    maximum: u64,
) -> Result<Vec<u8>> {
    let (valid_kind, oversized, result) = db
        .query_row(
            &format!(
                "SELECT typeof(value) IN ('text','blob'), coalesce(octet_length(value) > ?2, 0),
                    CASE WHEN typeof(value) IN ('text','blob')
                    AND octet_length(value) <= ?2 THEN value END
                 FROM main.{table} WHERE {rowid} = ?1"
            ),
            params![row, maximum.min(PROOF_BYTES)],
            |row| {
                let bytes = match row.get_ref(2)? {
                    rusqlite::types::ValueRef::Text(bytes)
                    | rusqlite::types::ValueRef::Blob(bytes) => Some(bytes.to_vec()),
                    _ => None,
                };
                Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?, bytes))
            },
        )
        .map_err(sql_error)?;
    if valid_kind && oversized {
        return Err(Error::Limit);
    }
    result.ok_or(Error::UnsupportedSource)
}
