//! Typed SQLite cell streams. See `docs/cloud/sqlite-native.md` for the format.

use crate::contract::encode_bounded;
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, DatabaseName, ErrorCode, types::ValueRef};
use serde::Serialize;
use std::collections::BTreeSet;
use std::time::Instant;

const ENVELOPE_BYTES: u64 = 64 * 1024;
const METADATA_BYTES: u64 = 1024 * 1024;
const FRAGMENT_BYTES: usize = 32 * 1024;

#[derive(Serialize)]
struct Column {
    name: String,
    declared_type: String,
}

struct ActiveTable {
    name: String,
    columns: Vec<Column>,
    select_row: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record<'a> {
    Header {
        format: &'static str,
        version: u32,
        text_encoding: &'a str,
    },
    Table {
        name: &'a str,
        columns: &'a [Column],
        key_columns: &'a [usize],
        order_columns: &'a [usize],
    },
    Row,
    Cell {
        #[serde(flatten)]
        value: Cell,
    },
    Data {
        base64: &'a str,
    },
    RowEnd,
    End,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Cell {
    Null,
    Integer { decimal: String },
    Real { bits: String },
    Text { bytes: u64 },
    Blob { bytes: u64 },
}

/// Writes selected tables from a caller-owned, read-only SQLite snapshot.
///
/// The provider owns authorization, connection safety settings, the transaction,
/// table selection, and logical row order. Row IDs only locate physical records.
/// Key and order declarations do not validate row uniqueness, nullability,
/// row ownership, or sorted order. The provider must prove these properties
/// before using declared keys as native identity.
/// Each successful sink call receives one complete LF-terminated envelope.
/// A sink error keeps its original closed error value. After any error, discard
/// the writer and its partial output; neither can resume a native read.
/// Run this synchronous writer on a bounded blocking worker, not a UI or async
/// runtime thread.
pub(crate) struct SqliteNativeWriter<'a, F>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    connection: &'a Connection,
    emit: F,
    maximum_bytes: u64,
    bytes_written: u64,
    metadata_bytes: u64,
    deadline: Instant,
    table: Option<ActiveTable>,
    declared_tables: BTreeSet<String>,
    failed: bool,
}

impl<'a, F> SqliteNativeWriter<'a, F>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    /// Emits the header without opening a database or starting a transaction.
    pub(crate) fn new(
        connection: &'a Connection,
        emit: F,
        maximum_bytes: u64,
        deadline: Instant,
    ) -> Result<Self> {
        let mut writer = Self {
            connection,
            emit,
            maximum_bytes,
            bytes_written: 0,
            metadata_bytes: 0,
            deadline,
            table: None,
            declared_tables: BTreeSet::new(),
            failed: false,
        };
        writer.check_deadline()?;
        let encoding: String = connection
            .query_row("PRAGMA main.encoding", [], |row| row.get(0))
            .map_err(sqlite_error)?;
        if !matches!(encoding.as_str(), "UTF-8" | "UTF-16le" | "UTF-16be") {
            return Err(Error::UnsupportedSource);
        }
        writer.record(
            Record::Header {
                format: "antiburn-sqlite-native",
                version: 1,
                text_encoding: &encoding,
            },
            true,
        )?;
        Ok(writer)
    }

    /// Declares all ordinary columns in SQLite column order, even for no rows.
    ///
    /// Table and column names must exactly match the source. Key and order lists
    /// each name distinct columns.
    /// Selecting another table ends the prior table. Repeated declarations,
    /// views, virtual tables, generated columns, and `WITHOUT ROWID` are invalid
    /// inputs or unsupported layouts. The writer never runs source DDL.
    pub(crate) fn begin_table(
        &mut self,
        name: &str,
        key_columns: &[&str],
        order_columns: &[&str],
    ) -> Result<()> {
        let result = self.declare_table(name, key_columns, order_columns);
        self.failed |= result.is_err();
        result
    }

    /// Validates an ordinary main table before publishing its declaration.
    fn declare_table(
        &mut self,
        name: &str,
        key_columns: &[&str],
        order_columns: &[&str],
    ) -> Result<()> {
        self.check_deadline()?;
        if name.contains('\0') {
            return Err(Error::InvalidSource);
        }
        if name.len() as u64 > ENVELOPE_BYTES {
            return Err(Error::Limit);
        }
        let folded_name = name.to_ascii_lowercase();
        if self.declared_tables.contains(&folded_name) {
            return Err(Error::InvalidSource);
        }
        let connection = self.connection;
        let mut layout = connection
            .prepare(
                "SELECT name, type, wr FROM pragma_table_list(?1)
                 WHERE schema = 'main' AND name = ?1 COLLATE NOCASE",
            )
            .map_err(sqlite_error)?;
        let mut rows = layout.query([name]).map_err(sqlite_error)?;
        let row = rows
            .next()
            .map_err(sqlite_error)?
            .ok_or(Error::InvalidSource)?;
        let source_name: String = row.get(0).map_err(sqlite_error)?;
        let table_type: String = row.get(1).map_err(sqlite_error)?;
        let without_rowid: i64 = row.get(2).map_err(sqlite_error)?;
        if source_name != name {
            return Err(Error::InvalidSource);
        }
        if table_type != "table" || without_rowid != 0 {
            return Err(Error::UnsupportedSource);
        }
        let mut statement = connection
            .prepare(
                "SELECT cid, name, type, hidden FROM pragma_table_xinfo(?1, 'main')
                 ORDER BY cid",
            )
            .map_err(sqlite_error)?;
        let mut rows = statement.query([name]).map_err(sqlite_error)?;
        let mut columns: Vec<Column> = Vec::new();
        let mut name_bytes = name.len() as u64;
        while let Some(row) = rows.next().map_err(sqlite_error)? {
            self.check_deadline()?;
            let hidden: i64 = row.get(3).map_err(sqlite_error)?;
            if hidden != 0 {
                return Err(Error::UnsupportedSource);
            }
            let cid: i64 = row.get(0).map_err(sqlite_error)?;
            if usize::try_from(cid).ok() != Some(columns.len()) {
                return Err(Error::InvalidSource);
            }
            let column_name = row.get_ref(1).map_err(sqlite_error)?;
            let declared_type = row.get_ref(2).map_err(sqlite_error)?;
            let column_name = column_name.as_str().map_err(|_| Error::InvalidSource)?;
            let declared_type = declared_type.as_str().map_err(|_| Error::InvalidSource)?;
            name_bytes = name_bytes
                .checked_add(column_name.len() as u64)
                .and_then(|bytes| bytes.checked_add(declared_type.len() as u64))
                .ok_or(Error::Limit)?;
            if name_bytes > ENVELOPE_BYTES {
                return Err(Error::Limit);
            }
            if column_name.contains('\0')
                || columns
                    .iter()
                    .any(|column| column.name.eq_ignore_ascii_case(column_name))
            {
                return Err(Error::InvalidSource);
            }
            columns.push(Column {
                name: column_name.to_owned(),
                declared_type: declared_type.to_owned(),
            });
        }
        if columns.is_empty() {
            return Err(Error::InvalidSource);
        }
        let rowid = ["_rowid_", "rowid", "oid"]
            .into_iter()
            .find(|alias| {
                columns
                    .iter()
                    .all(|column| !column.name.eq_ignore_ascii_case(alias))
            })
            .ok_or(Error::UnsupportedSource)?;
        let keys = column_indices(&columns, key_columns)?;
        let order = column_indices(&columns, order_columns)?;
        // Only scalar values leave SQLite through this query. TEXT and BLOB
        // markers let the incremental handle read their original encoded bytes.
        let values: Vec<_> = columns
            .iter()
            .map(|column| {
                let quoted = quote_identifier(&column.name);
                format!(
                    "CASE typeof({quoted}) WHEN 'text' THEN 'text' \
                     WHEN 'blob' THEN 'blob' ELSE {quoted} END"
                )
            })
            .collect();
        let select_row = format!(
            "SELECT {} FROM main.{} WHERE {} = ?1",
            values.join(","),
            quote_identifier(name),
            quote_identifier(rowid),
        );
        self.record(
            Record::Table {
                name,
                columns: &columns,
                key_columns: &keys,
                order_columns: &order,
            },
            true,
        )?;
        self.declared_tables.insert(folded_name);
        self.table = Some(ActiveTable {
            name: name.to_owned(),
            columns,
            select_row,
        });
        Ok(())
    }

    /// Emits one complete row in column order, using only a physical locator.
    pub(crate) fn write_row(&mut self, rowid: i64) -> Result<()> {
        let result = self.write_active_row(rowid);
        self.failed |= result.is_err();
        result
    }

    /// Takes the active table out while row emission borrows the writer mutably.
    /// Restores it on success or error; `write_row` blocks reuse after an error.
    fn write_active_row(&mut self, rowid: i64) -> Result<()> {
        self.check_deadline()?;
        let table = self.table.take().ok_or(Error::InvalidSource)?;
        let result = self.write_table_row(&table, rowid);
        self.table = Some(table);
        result
    }

    /// Emits scalars from the SELECT, which returns only type markers for TEXT
    /// and BLOB. Native bytes come from read-only incremental handles.
    fn write_table_row(&mut self, table: &ActiveTable, rowid: i64) -> Result<()> {
        let connection = self.connection;
        let mut statement = connection
            .prepare(&table.select_row)
            .map_err(sqlite_error)?;
        let mut rows = statement.query([rowid]).map_err(sqlite_error)?;
        let row = rows
            .next()
            .map_err(sqlite_error)?
            .ok_or(Error::InvalidSource)?;
        self.record(Record::Row, false)?;
        for (index, column) in table.columns.iter().enumerate() {
            self.check_deadline()?;
            let cell = match row.get_ref(index).map_err(sqlite_error)? {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(value) => Cell::Integer {
                    decimal: value.to_string(),
                },
                ValueRef::Real(value) => Cell::Real {
                    bits: format!("{:016x}", value.to_bits()),
                },
                ValueRef::Text(b"text") => {
                    self.write_bytes(table, column, rowid, true)?;
                    continue;
                }
                ValueRef::Text(b"blob") => {
                    self.write_bytes(table, column, rowid, false)?;
                    continue;
                }
                _ => return Err(Error::InvalidSource),
            };
            self.record(Record::Cell { value: cell }, false)?;
        }
        self.record(Record::RowEnd, false)
    }

    /// Reads TEXT and BLOB without converting the native text encoding.
    fn write_bytes(
        &mut self,
        table: &ActiveTable,
        column: &Column,
        rowid: i64,
        text: bool,
    ) -> Result<()> {
        self.check_deadline()?;
        let connection = self.connection;
        let blob = connection
            .blob_open(DatabaseName::Main, &table.name, &column.name, rowid, true)
            .map_err(sqlite_error)?;
        let bytes = blob.len() as u64;
        let value = if text {
            Cell::Text { bytes }
        } else {
            Cell::Blob { bytes }
        };
        self.record(Record::Cell { value }, false)?;
        let mut buffer = [0_u8; FRAGMENT_BYTES];
        let mut offset = 0;
        while offset < blob.len() {
            self.check_deadline()?;
            let length = FRAGMENT_BYTES.min(blob.len() - offset);
            blob.read_at_exact(&mut buffer[..length], offset)
                .map_err(sqlite_error)?;
            let encoded = STANDARD.encode(&buffer[..length]);
            self.record(Record::Data { base64: &encoded }, false)?;
            offset += length;
        }
        blob.close().map_err(sqlite_error)
    }

    /// Emits the required final record. An errored writer cannot complete.
    pub(crate) fn finish(mut self) -> Result<()> {
        self.record(Record::End, false)
    }

    fn check_deadline(&self) -> Result<()> {
        if self.failed {
            return Err(Error::InvalidSource);
        }
        if Instant::now() >= self.deadline {
            return Err(Error::Limit);
        }
        Ok(())
    }

    /// Checks output bounds before the sink receives an envelope.
    fn record(&mut self, record: Record<'_>, metadata: bool) -> Result<()> {
        self.check_deadline()?;
        let mut bytes = encode_bounded(&record, ENVELOPE_BYTES - 1)?;
        bytes.push(b'\n');
        let length = bytes.len() as u64;
        if length > self.maximum_bytes.saturating_sub(self.bytes_written)
            || (metadata && length > METADATA_BYTES.saturating_sub(self.metadata_bytes))
        {
            return Err(Error::Limit);
        }
        self.check_deadline()?;
        (self.emit)(&bytes)?;
        self.bytes_written += length;
        if metadata {
            self.metadata_bytes += length;
        }
        self.check_deadline()
    }
}

fn column_indices(columns: &[Column], names: &[&str]) -> Result<Vec<usize>> {
    let mut indices = Vec::new();
    for name in names {
        let index = columns
            .iter()
            .position(|column| column.name == *name)
            .ok_or(Error::InvalidSource)?;
        if indices.contains(&index) {
            return Err(Error::InvalidSource);
        }
        indices.push(index);
    }
    Ok(indices)
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

/// Removes SQLite details so values and source paths cannot enter errors.
fn sqlite_error(error: rusqlite::Error) -> Error {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => Error::Busy,
        Some(ErrorCode::OperationInterrupted | ErrorCode::TooBig | ErrorCode::OutOfMemory) => {
            Error::Limit
        }
        _ => Error::InvalidSource,
    }
}

#[cfg(test)]
mod tests;
