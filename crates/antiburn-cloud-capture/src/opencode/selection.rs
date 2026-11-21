//! Explicit OpenCode ownership and dependency selection, not a foreign-key crawl.

use super::references::{self, References, Shape};
use super::{
    MAX_ID_BYTES, MAX_ROWS, MAX_SCHEMA_BYTES, OpenCodeExport, bounded_text, check_deadline,
    sql_error, validate_id,
};
use crate::{Error, Limits, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const MAX_MEDIA_CELL_BYTES: usize = 64 * 1024 * 1024;
const MAX_MEDIA_TOTAL_BYTES: usize = 256 * 1024 * 1024;

struct TableSpec {
    name: &'static str,
    owner: &'static str,
    keys: &'static [&'static str],
    order: &'static [&'static str],
    required: &'static [&'static str],
}

const TABLES: &[TableSpec] = &[
    TableSpec {
        name: "session_v2",
        owner: "id",
        keys: &["id"],
        order: &["id"],
        required: &["id"],
    },
    TableSpec {
        name: "session_message",
        owner: "session_id",
        keys: &["id"],
        order: &["seq", "id"],
        required: &["id", "session_id", "type", "seq", "data"],
    },
    TableSpec {
        name: "session_part",
        owner: "session_id",
        keys: &["message_id", "ordinal"],
        order: &["message_id", "ordinal"],
        required: &["session_id", "message_id", "ordinal", "type", "data"],
    },
    TableSpec {
        name: "event_sequence",
        owner: "aggregate_id",
        keys: &["aggregate_id"],
        order: &["aggregate_id"],
        required: &["aggregate_id", "seq"],
    },
    TableSpec {
        name: "event",
        owner: "aggregate_id",
        keys: &["id"],
        order: &["seq", "id"],
        required: &["id", "aggregate_id", "seq", "type", "data"],
    },
    TableSpec {
        name: "session_inbox",
        owner: "session_id",
        keys: &["id"],
        order: &["enqueued_seq", "id"],
        required: &["id", "session_id", "type", "enqueued_seq", "payload"],
    },
    TableSpec {
        name: "session_pending",
        owner: "session_id",
        keys: &["id"],
        order: &["admitted_seq", "id"],
        required: &["id", "session_id", "type", "admitted_seq", "data"],
    },
    TableSpec {
        name: "instruction_entry",
        owner: "session_id",
        keys: &["session_id", "key"],
        order: &["key"],
        required: &["session_id", "key", "value", "removed"],
    },
    TableSpec {
        name: "instruction_state",
        owner: "session_id",
        keys: &["session_id"],
        order: &["session_id"],
        required: &["session_id", "initial_values", "current_values"],
    },
    TableSpec {
        name: "job",
        owner: "session_id",
        keys: &["notification_id"],
        order: &["notification_id"],
        required: &["notification_id", "session_id", "output", "error"],
    },
    TableSpec {
        name: "blob_ref",
        owner: "session_id",
        keys: &["session_id", "message_id", "hash"],
        order: &["seq", "message_id", "hash"],
        required: &["session_id", "message_id", "hash", "seq"],
    },
];

/// Ordered native row identifiers. They are transient read locators, not keys.
pub(super) struct SelectedTable {
    pub name: &'static str,
    pub keys: &'static [&'static str],
    pub order: &'static [&'static str],
    pub rows: Vec<i64>,
}

/// Validated capabilities from the caller's transaction.
///
/// Extra ordinary columns remain native source. Absent optional tables are not
/// errors, but present malformed tables and unknown session-linked tables are.
pub(super) struct Schema {
    columns: BTreeMap<String, BTreeSet<String>>,
}

impl Schema {
    /// Inspect bounded metadata without reading native payload columns.
    pub(super) fn read(
        connection: &Connection,
        limits: &Limits,
        deadline: Instant,
    ) -> Result<Self> {
        let mut schema_bytes: usize = connection
            .query_row(
                "SELECT COALESCE(sum(octet_length(sql)), 0) FROM sqlite_schema",
                [],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if schema_bytes > MAX_SCHEMA_BYTES {
            return Err(Error::Limit);
        }
        let mut statement = connection
            .prepare(
                "SELECT CASE WHEN octet_length(name) <= 256 THEN name END, type
                 FROM sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'
                 ORDER BY name COLLATE BINARY LIMIT ?1",
            )
            .map_err(sql_error)?;
        let mut rows = statement
            .query([limits.discovery_entries + 1])
            .map_err(sql_error)?;
        let mut columns = BTreeMap::new();
        while let Some(row) = rows.next().map_err(sql_error)? {
            check_deadline(deadline)?;
            if columns.len() == limits.discovery_entries {
                return Err(Error::Limit);
            }
            let name: Option<String> = row.get(0).map_err(sql_error)?;
            let name = name.ok_or(Error::Limit)?;
            let kind: String = row.get(1).map_err(sql_error)?;
            if excluded_table(&name) {
                columns.insert(name, BTreeSet::new());
                continue;
            }
            let fields = table_columns(connection, &name, &mut schema_bytes)?;
            let known = TABLES.iter().any(|spec| spec.name == name)
                || matches!(name.as_str(), "blob" | "instruction_blob");
            if known {
                if kind != "table" {
                    return Err(Error::UnsupportedSource);
                }
                ordinary_table(connection, &name)?;
            } else if name.starts_with("session_")
                || fields.iter().any(|field| {
                    matches!(
                        field.as_str(),
                        "session_id" | "aggregate_id" | "parent_session_id" | "child_session_id"
                    )
                })
                || references_owned_table(connection, &name)?
            {
                return Err(Error::UnsupportedSource);
            }
            columns.insert(name, fields);
        }
        let schema = Self { columns };
        for name in ["session_v2", "session_message"] {
            if !schema.has(name) {
                return Err(Error::UnsupportedSource);
            }
        }
        for spec in TABLES.iter().filter(|spec| schema.has(spec.name)) {
            schema.require(spec.name, spec.required)?;
            if !unique_key(connection, spec.name, spec.keys)? {
                return Err(Error::UnsupportedSource);
            }
        }
        for (table, fields) in [
            ("blob", &["hash", "data"][..]),
            ("instruction_blob", &["hash", "value"][..]),
        ] {
            if schema.has(table) {
                schema.require(table, fields)?;
                if !unique_key(connection, table, &["hash"])? {
                    return Err(Error::UnsupportedSource);
                }
            }
        }
        for (table, owner, sequence) in [
            ("session_message", "session_id", "seq"),
            ("event", "aggregate_id", "seq"),
        ] {
            if schema.has(table) && !unique_key(connection, table, &[owner, sequence])? {
                return Err(Error::UnsupportedSource);
            }
        }
        if schema.has("event") && !schema.has("event_sequence") {
            return Err(Error::UnsupportedSource);
        }
        Ok(schema)
    }

    /// Distinguish absent optional tables from present validated capabilities.
    pub(super) fn has(&self, name: &str) -> bool {
        self.columns.contains_key(name)
    }

    fn require(&self, table: &str, fields: &[&str]) -> Result<()> {
        if !fields
            .iter()
            .all(|field| self.columns[table].contains(*field))
        {
            return Err(Error::UnsupportedSource);
        }
        Ok(())
    }

    /// Reject missing or oversized discovery IDs without materializing them.
    pub(super) fn validate_identities(&self, connection: &Connection) -> Result<()> {
        let invalid: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_v2
                 WHERE typeof(id) != 'text' OR octet_length(id) NOT BETWEEN 1 AND ?1)",
                [MAX_ID_BYTES],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if invalid {
            return Err(Error::InvalidSource);
        }
        Ok(())
    }

    /// Read this source-owned CWD only, never project tables or parent sessions.
    pub(super) fn project_evidence(
        &self,
        connection: &Connection,
        id: &str,
    ) -> Result<crate::ProjectEvidence> {
        if !self.columns["session_v2"].contains("directory") {
            return Ok(crate::ProjectEvidence::unknown());
        }
        let value: Option<Option<String>> = connection
            .query_row(
                "SELECT CASE WHEN typeof(directory) = 'text'
                    AND octet_length(directory) BETWEEN 1 AND 4096 THEN directory END
                 FROM session_v2 WHERE id = ?1 COLLATE BINARY",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        let value = value.ok_or(Error::SourceChanged)?;
        Ok(value.map_or_else(crate::ProjectEvidence::unknown, |path| {
            crate::ProjectEvidence::recorded(&path)
        }))
    }

    /// Read direct edges only; never query the referenced parent's source.
    pub(super) fn relationships(
        &self,
        connection: &Connection,
        id: &str,
    ) -> Result<OpenCodeExport> {
        let rowid = connection
            .query_row(
                "SELECT rowid FROM session_v2 WHERE id = ?1 COLLATE BINARY",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?
            .ok_or(Error::SourceChanged)?;
        let read = |column: &str| -> Result<Option<String>> {
            if !self.columns["session_v2"].contains(column) {
                return Ok(None);
            }
            let value = bounded_text(connection, "session_v2", column, rowid)?;
            if let Some(value) = &value {
                validate_id(value)?;
                if value == id {
                    return Err(Error::InvalidSource);
                }
            }
            Ok(value)
        };
        let parent_session_id = read("parent_id")?;
        let fork_session_id = read("fork_session_id")?;
        if parent_session_id.is_some() && fork_session_id.is_some() {
            return Err(Error::InvalidSource);
        }
        Ok(OpenCodeExport {
            parent_session_id,
            fork_session_id,
        })
    }
}

/// Select complete owned row sets and explicitly proved shared values.
///
/// Missing native dependency rows remain absent. Present oversized media, unsafe
/// ownership, or an incomplete reference scan fails the whole export.
pub(super) fn select(
    connection: &Connection,
    schema: &Schema,
    id: &str,
    limits: &Limits,
    deadline: Instant,
) -> Result<Vec<SelectedTable>> {
    let mut selected = Vec::new();
    let mut count = 0;
    for spec in TABLES.iter().filter(|spec| schema.has(spec.name)) {
        check_deadline(deadline)?;
        validate_owned_rows(connection, spec, id)?;
        let order = order_sql(spec.order);
        let query = format!(
            "SELECT rowid FROM \"{}\" WHERE \"{}\" = ?1 COLLATE BINARY
             ORDER BY {order} LIMIT ?2",
            spec.name, spec.owner,
        );
        let mut statement = connection.prepare(&query).map_err(sql_error)?;
        let rows = statement
            .query_map(params![id, MAX_ROWS.saturating_sub(count) + 1], |row| {
                row.get(0)
            })
            .map_err(sql_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        count += rows.len();
        if count > MAX_ROWS {
            return Err(Error::Limit);
        }
        selected.push(SelectedTable {
            name: spec.name,
            keys: spec.keys,
            order: spec.order,
            rows,
        });
    }
    if schema.has("session_part") {
        let invalid: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_part p WHERE p.session_id = ?1 COLLATE BINARY
             AND NOT EXISTS(SELECT 1 FROM session_message m
                            WHERE m.id = p.message_id COLLATE BINARY
                              AND m.session_id = ?1 COLLATE BINARY))",
                [id],
                |row| row.get(0),
            )
            .map_err(sql_error)?;
        if invalid {
            return Err(Error::InvalidSource);
        }
    }
    if schema.has("blob_ref") {
        validate_blob_ref_owners(connection, schema, id)?;
    }
    let mut references = References::new(limits.source_bytes, deadline);
    for table in &selected {
        for rowid in &table.rows {
            scan_row(connection, table.name, *rowid, id, &mut references)?;
        }
    }
    let mut media_bytes = 0usize;
    for (table, column, hashes) in [
        ("instruction_blob", "value", &references.instructions),
        ("blob", "data", &references.media),
    ] {
        check_deadline(deadline)?;
        if !schema.has(table) {
            if !hashes.is_empty() {
                return Err(Error::UnsupportedSource);
            }
            continue;
        }
        let mut statement = connection
            .prepare(&format!(
                "SELECT rowid FROM \"{table}\" WHERE hash = ?1 COLLATE BINARY"
            ))
            .map_err(sql_error)?;
        let mut rows = Vec::new();
        for hash in hashes {
            check_deadline(deadline)?;
            if let Some(rowid) = statement
                .query_row([hash], |row| row.get(0))
                .optional()
                .map_err(sql_error)?
            {
                if count == MAX_ROWS {
                    return Err(Error::Limit);
                }
                count += 1;
                rows.push(rowid);
                let kind: String = connection
                    .query_row(
                        &format!("SELECT typeof(\"{column}\") FROM \"{table}\" WHERE rowid = ?1"),
                        [rowid],
                        |row| row.get(0),
                    )
                    .map_err(sql_error)?;
                if table == "blob" && kind != "blob" {
                    return Err(Error::InvalidSource);
                }
                if table == "blob" {
                    let blob = connection
                        .blob_open(rusqlite::DatabaseName::Main, table, column, rowid, true)
                        .map_err(sql_error)?;
                    let length = blob.len();
                    media_bytes = media_bytes.checked_add(length).ok_or(Error::Limit)?;
                    if length > MAX_MEDIA_CELL_BYTES
                        || media_bytes > MAX_MEDIA_TOTAL_BYTES
                        || media_bytes as u64 > limits.source_bytes
                    {
                        return Err(Error::Limit);
                    }
                }
            }
        }
        selected.push(SelectedTable {
            name: table,
            keys: &["hash"],
            order: &["hash"],
            rows,
        });
    }
    Ok(selected)
}

fn scan_row(
    connection: &Connection,
    table: &str,
    rowid: i64,
    id: &str,
    refs: &mut References,
) -> Result<()> {
    if table == "blob_ref" {
        let hash = bounded_text(connection, table, "hash", rowid)?.ok_or(Error::InvalidSource)?;
        return refs.add_media(&hash);
    }
    if table == "instruction_state" {
        for column in ["initial_values", "current_values"] {
            references::scan(
                connection,
                table,
                column,
                rowid,
                Shape::InstructionValues,
                refs,
                None,
            )?;
        }
        return Ok(());
    }
    let (column, shape) = match table {
        "session_message" | "session_part" | "session_inbox" | "session_pending" | "event" => {
            let kind =
                bounded_text(connection, table, "type", rowid)?.ok_or(Error::InvalidSource)?;
            let kind = if table == "event" {
                event_name(&kind)
            } else {
                kind.as_str()
            };
            match (table, kind) {
                ("session_message", "user") => ("data", Shape::User),
                ("session_message", "assistant") => ("data", Shape::Assistant),
                ("session_part", "tool") => ("data", Shape::ToolPart),
                ("session_inbox", "user") => ("payload", Shape::User),
                ("session_pending", "user") => ("data", Shape::User),
                ("event", "session.instructions.updated") => ("data", Shape::InstructionsEvent),
                ("event", "session.forked") => ("data", Shape::ForkEvent),
                ("event", "session.tool.success" | "session.tool.failed") => {
                    ("data", Shape::ToolEvent)
                }
                ("event", "session.message.content.updated") => ("data", Shape::Assistant),
                ("event", "session.inbox.enqueued") => ("data", Shape::InboxEvent),
                _ => return Ok(()),
            }
        }
        _ => return Ok(()),
    };
    references::scan(
        connection,
        table,
        column,
        rowid,
        shape,
        refs,
        (table == "event").then_some(id),
    )
}

/// A retained reference may outlive its target, but cannot reassign an existing
/// message or queued input from another session to the selected session.
fn validate_blob_ref_owners(connection: &Connection, schema: &Schema, id: &str) -> Result<()> {
    for table in ["session_message", "session_inbox", "session_pending"] {
        if !schema.has(table) {
            continue;
        }
        let query = format!(
            "SELECT EXISTS(
               SELECT 1 FROM blob_ref r WHERE r.session_id = ?1 COLLATE BINARY
                AND EXISTS(SELECT 1 FROM \"{table}\" t
                            WHERE t.id = r.message_id COLLATE BINARY
                              AND (typeof(t.session_id) != 'text'
                                   OR t.session_id != ?1 COLLATE BINARY)))"
        );
        if connection
            .query_row(&query, [id], |row| row.get::<_, bool>(0))
            .map_err(sql_error)?
        {
            return Err(Error::InvalidSource);
        }
    }
    Ok(())
}

/// Native event types include a positive schema-version suffix, not a runtime
/// version. Strip it only for reference-field selection, never in exported cells.
fn event_name(kind: &str) -> &str {
    match kind.rsplit_once('.') {
        Some((name, version))
            if !version.is_empty()
                && version.bytes().all(|byte| byte.is_ascii_digit())
                && version.bytes().any(|byte| byte != b'0') =>
        {
            name
        }
        _ => kind,
    }
}

fn validate_owned_rows(connection: &Connection, spec: &TableSpec, id: &str) -> Result<()> {
    let mut invalid = Vec::new();
    for column in spec.keys.iter().copied().chain([spec.owner]) {
        if column == "ordinal" {
            continue;
        }
        let bound = if column == "key" { 4096 } else { MAX_ID_BYTES };
        invalid.push(format!(
            "(typeof(\"{column}\") != 'text' OR octet_length(\"{column}\") NOT BETWEEN 1 AND {bound})"
        ));
    }
    for column in spec.required {
        if matches!(*column, "seq" | "ordinal" | "enqueued_seq" | "admitted_seq") {
            invalid.push(format!(
                "(typeof(\"{column}\") != 'integer' OR \"{column}\" < 0)"
            ));
        }
    }
    let query = format!(
        "SELECT EXISTS(SELECT 1 FROM \"{}\" WHERE \"{}\" = ?1 COLLATE BINARY AND ({}))",
        spec.name,
        spec.owner,
        invalid.join(" OR "),
    );
    if connection
        .query_row(&query, [id], |row| row.get::<_, bool>(0))
        .map_err(sql_error)?
    {
        return Err(Error::InvalidSource);
    }
    Ok(())
}

fn order_sql(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| format!("\"{column}\" COLLATE BINARY"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn table_columns(
    connection: &Connection,
    table: &str,
    bytes: &mut usize,
) -> Result<BTreeSet<String>> {
    let mut statement = connection
        .prepare(
            "SELECT CASE WHEN octet_length(name) <= 256 THEN name END, hidden
         FROM pragma_table_xinfo(?1) LIMIT 257",
        )
        .map_err(sql_error)?;
    let mut rows = statement.query([table]).map_err(sql_error)?;
    let mut columns = BTreeSet::new();
    while let Some(row) = rows.next().map_err(sql_error)? {
        let name: Option<String> = row.get(0).map_err(sql_error)?;
        let name = name.ok_or(Error::Limit)?;
        *bytes = bytes.checked_add(name.len()).ok_or(Error::Limit)?;
        if *bytes > MAX_SCHEMA_BYTES || columns.len() == 256 {
            return Err(Error::Limit);
        }
        columns.insert(name);
    }
    Ok(columns)
}

fn ordinary_table(connection: &Connection, table: &str) -> Result<()> {
    let ordinary: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_list
         WHERE schema = 'main' AND name = ?1 COLLATE BINARY AND type = 'table' AND wr = 0)",
            [table],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    // All provider SQL and incremental BLOB reads must identify the same physical
    // row as the generic writer. An ordinary rowid column would shadow that row.
    let unsafe_columns: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_xinfo(?1)
             WHERE hidden != 0 OR name = 'rowid' COLLATE NOCASE)",
            [table],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    if !ordinary || unsafe_columns {
        return Err(Error::UnsupportedSource);
    }
    Ok(())
}

fn unique_key(connection: &Connection, table: &str, columns: &[&str]) -> Result<bool> {
    let mut statement = connection
        .prepare(
            "SELECT CASE WHEN octet_length(name) <= 256 THEN name END
         FROM pragma_index_list(?1) WHERE \"unique\" = 1 AND partial = 0 LIMIT 257",
        )
        .map_err(sql_error)?;
    let indexes = statement
        .query_map([table], |row| row.get::<_, Option<String>>(0))
        .map_err(sql_error)?;
    for (number, index) in indexes.enumerate() {
        if number == 256 {
            return Err(Error::Limit);
        }
        let index = index.map_err(sql_error)?.ok_or(Error::Limit)?;
        let mut statement = connection
            .prepare(
                "SELECT name FROM pragma_index_xinfo(?1) WHERE key = 1 ORDER BY seqno LIMIT 257",
            )
            .map_err(sql_error)?;
        let names = statement
            .query_map([index], |row| row.get::<_, Option<String>>(0))
            .map_err(sql_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sql_error)?;
        if names
            .iter()
            .map(Option::as_deref)
            .eq(columns.iter().copied().map(Some))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn excluded_table(name: &str) -> bool {
    matches!(name, "session" | "message" | "part" | "schedule")
}

fn references_owned_table(connection: &Connection, table: &str) -> Result<bool> {
    let mut statement = connection
        .prepare("SELECT \"table\" FROM pragma_foreign_key_list(?1) LIMIT 257")
        .map_err(sql_error)?;
    let rows = statement
        .query_map([table], |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    for (number, name) in rows.enumerate() {
        if number == 256 {
            return Err(Error::Limit);
        }
        let name = name.map_err(sql_error)?;
        if TABLES
            .iter()
            .any(|spec| spec.name.eq_ignore_ascii_case(&name))
            || ["blob", "instruction_blob"]
                .iter()
                .any(|table| table.eq_ignore_ascii_case(&name))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
