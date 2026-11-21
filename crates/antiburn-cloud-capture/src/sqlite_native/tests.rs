use super::SqliteNativeWriter;
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use rusqlite::{Connection, DatabaseName, OpenFlags, params, types::Value};
use serde::Deserialize;
use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
enum NativeCell {
    Null,
    Integer(i64),
    Real(u64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct NativeColumn {
    name: String,
    declared_type: String,
}

#[derive(Debug, PartialEq, Eq)]
struct NativeTable {
    name: String,
    columns: Vec<NativeColumn>,
    key_columns: Vec<usize>,
    order_columns: Vec<usize>,
    rows: Vec<Vec<NativeCell>>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Envelope {
    Header {
        format: String,
        version: u32,
        text_encoding: String,
    },
    Table {
        name: String,
        columns: Vec<NativeColumn>,
        key_columns: Vec<usize>,
        order_columns: Vec<usize>,
    },
    Row,
    Cell {
        r#type: String,
        decimal: Option<String>,
        bits: Option<String>,
        bytes: Option<u64>,
    },
    Data {
        base64: String,
    },
    RowEnd,
    End,
}

/// This oracle reads envelopes only. It neither uses writer helpers nor imports
/// cells into SQLite, where column affinity could change their storage class.
fn decode(bytes: &[u8]) -> std::result::Result<(String, Vec<NativeTable>), String> {
    if !bytes.ends_with(b"\n") {
        return Err("missing final LF".into());
    }
    let mut records = VecDeque::new();
    for line in bytes[..bytes.len() - 1].split(|byte| *byte == b'\n') {
        if line.is_empty() || line.len() + 1 > 65_536 {
            return Err("invalid envelope length".into());
        }
        let envelope: Envelope = serde_json::from_slice(line).map_err(|error| error.to_string())?;
        let object: serde_json::Value =
            serde_json::from_slice(line).map_err(|error| error.to_string())?;
        let expected: &[&str] = match &envelope {
            Envelope::Header { .. } => &["kind", "format", "version", "text_encoding"],
            Envelope::Table { .. } => &["kind", "name", "columns", "key_columns", "order_columns"],
            Envelope::Row | Envelope::RowEnd | Envelope::End => &["kind"],
            Envelope::Data { .. } => &["kind", "base64"],
            Envelope::Cell { r#type, .. } => match r#type.as_str() {
                "null" => &["kind", "type"],
                "integer" => &["kind", "type", "decimal"],
                "real" => &["kind", "type", "bits"],
                "text" | "blob" => &["kind", "type", "bytes"],
                _ => return Err("unknown cell type".into()),
            },
        };
        let keys: BTreeSet<_> = object
            .as_object()
            .ok_or("not an object")?
            .keys()
            .map(String::as_str)
            .collect();
        if keys != expected.iter().copied().collect() {
            return Err("invalid envelope fields".into());
        }
        records.push_back(envelope);
    }
    let encoding = match records.pop_front() {
        Some(Envelope::Header {
            format,
            version: 1,
            text_encoding,
        }) if format == "antiburn-sqlite-native"
            && matches!(text_encoding.as_str(), "UTF-8" | "UTF-16le" | "UTF-16be") =>
        {
            text_encoding
        }
        _ => return Err("missing or invalid header".into()),
    };
    let mut tables = Vec::new();
    let mut names = BTreeSet::new();
    loop {
        let mut table = match records.pop_front() {
            Some(Envelope::Table {
                name,
                columns,
                key_columns,
                order_columns,
            }) => {
                if !names.insert(name.to_ascii_lowercase()) || columns.is_empty() {
                    return Err("invalid table declaration".into());
                }
                let mut column_names = BTreeSet::new();
                if columns
                    .iter()
                    .any(|column| !column_names.insert(column.name.to_ascii_lowercase()))
                {
                    return Err("duplicate column".into());
                }
                for indices in [&key_columns, &order_columns] {
                    let distinct: BTreeSet<_> = indices.iter().copied().collect();
                    if distinct.len() != indices.len()
                        || indices.iter().any(|index| *index >= columns.len())
                    {
                        return Err("invalid column index".into());
                    }
                }
                NativeTable {
                    name,
                    columns,
                    key_columns,
                    order_columns,
                    rows: Vec::new(),
                }
            }
            Some(Envelope::End) if records.is_empty() => return Ok((encoding, tables)),
            _ => return Err("expected table or final end".into()),
        };
        while matches!(records.front(), Some(Envelope::Row)) {
            records.pop_front();
            let mut cells = Vec::new();
            for _ in &table.columns {
                let Some(Envelope::Cell {
                    r#type,
                    decimal,
                    bits,
                    bytes,
                }) = records.pop_front()
                else {
                    return Err("missing cell".into());
                };
                let cell = match r#type.as_str() {
                    "null" if decimal.is_none() && bits.is_none() && bytes.is_none() => {
                        NativeCell::Null
                    }
                    "integer" if bits.is_none() && bytes.is_none() => {
                        let decimal = decimal.ok_or("missing integer")?;
                        let integer: i64 = decimal.parse().map_err(|_| "invalid integer")?;
                        if integer.to_string() != decimal {
                            return Err("noncanonical integer".into());
                        }
                        NativeCell::Integer(integer)
                    }
                    "real" if decimal.is_none() && bytes.is_none() => {
                        let bits = bits.ok_or("missing real")?;
                        if bits.len() != 16
                            || !bits
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                        {
                            return Err("invalid real bits".into());
                        }
                        NativeCell::Real(
                            u64::from_str_radix(&bits, 16).map_err(|_| "invalid real")?,
                        )
                    }
                    "text" | "blob" if decimal.is_none() && bits.is_none() => {
                        let mut remaining = bytes.ok_or("missing byte length")?;
                        let mut decoded = Vec::new();
                        while remaining != 0 {
                            let Some(Envelope::Data { base64 }) = records.pop_front() else {
                                return Err("missing cell data".into());
                            };
                            let fragment =
                                STANDARD.decode(&base64).map_err(|_| "invalid base64")?;
                            if fragment.len() as u64 != remaining.min(32_768)
                                || STANDARD.encode(&fragment) != base64
                            {
                                return Err("invalid fragment length or padding".into());
                            }
                            remaining -= fragment.len() as u64;
                            decoded.extend_from_slice(&fragment);
                        }
                        if r#type == "text" {
                            NativeCell::Text(decoded)
                        } else {
                            NativeCell::Blob(decoded)
                        }
                    }
                    _ => return Err("invalid cell".into()),
                };
                cells.push(cell);
            }
            if !matches!(records.pop_front(), Some(Envelope::RowEnd)) {
                return Err("missing row_end".into());
            }
            table.rows.push(cells);
        }
        tables.push(table);
    }
}

fn snapshot(connection: &Connection) {
    connection
        .execute_batch("PRAGMA query_only = ON; BEGIN")
        .unwrap();
}

type TableSelection<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [i64]);

fn capture(connection: &Connection, tables: &[TableSelection<'_>]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut writer = SqliteNativeWriter::new(
        connection,
        |bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        },
        64 * 1024 * 1024,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    for (name, keys, order, rowids) in tables {
        writer.begin_table(name, keys, order).unwrap();
        for rowid in *rowids {
            writer.write_row(*rowid).unwrap();
        }
    }
    writer.finish().unwrap();
    output
}

fn sql_name(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn observable_table(
    connection: &Connection,
    name: &str,
    keys: Vec<usize>,
    order: Vec<usize>,
    rowids: &[i64],
) -> NativeTable {
    let columns: Vec<NativeColumn> = connection
        .prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap()
        .query_map([name], |row| {
            Ok(NativeColumn {
                name: row.get(0)?,
                declared_type: row.get(1)?,
            })
        })
        .unwrap()
        .map(|column| column.unwrap())
        .collect();
    let rows = rowids
        .iter()
        .map(|rowid| {
            columns
                .iter()
                .map(|column| {
                    let column = sql_name(&column.name);
                    let query = format!(
                        "SELECT typeof({column}), \
                         CASE WHEN typeof({column}) IN ('integer', 'real') THEN {column} END, \
                         CASE WHEN typeof({column}) IN ('text', 'blob') THEN CAST({column} AS BLOB) END \
                         FROM {} WHERE _rowid_ = ?1",
                        sql_name(name),
                    );
                    connection
                        .query_row(&query, [rowid], |row| {
                            let kind: String = row.get(0)?;
                            Ok(match kind.as_str() {
                                "null" => NativeCell::Null,
                                "integer" => NativeCell::Integer(row.get(1)?),
                                "real" => NativeCell::Real(row.get::<_, f64>(1)?.to_bits()),
                                "text" => NativeCell::Text(row.get(2)?),
                                "blob" => NativeCell::Blob(row.get(2)?),
                                _ => panic!("unexpected SQLite storage class"),
                            })
                        })
                        .unwrap()
                })
                .collect()
        })
        .collect();
    NativeTable {
        name: name.to_owned(),
        columns,
        key_columns: keys,
        order_columns: order,
        rows,
    }
}

#[test]
fn fixed_field_order_and_scalar_spelling() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sample (key TEXT, payload); INSERT INTO sample VALUES ('x', -9223372036854775808)")
        .unwrap();
    snapshot(&connection);
    let bytes = capture(&connection, &[("sample", &["key"], &["key"], &[1])]);
    assert_eq!(
        String::from_utf8(bytes.clone()).unwrap(),
        concat!(
            "{\"kind\":\"header\",\"format\":\"antiburn-sqlite-native\",\"version\":1,\"text_encoding\":\"UTF-8\"}\n",
            "{\"kind\":\"table\",\"name\":\"sample\",\"columns\":[{\"name\":\"key\",\"declared_type\":\"TEXT\"},{\"name\":\"payload\",\"declared_type\":\"\"}],\"key_columns\":[0],\"order_columns\":[0]}\n",
            "{\"kind\":\"row\"}\n",
            "{\"kind\":\"cell\",\"type\":\"text\",\"bytes\":1}\n",
            "{\"kind\":\"data\",\"base64\":\"eA==\"}\n",
            "{\"kind\":\"cell\",\"type\":\"integer\",\"decimal\":\"-9223372036854775808\"}\n",
            "{\"kind\":\"row_end\"}\n",
            "{\"kind\":\"end\"}\n",
        ),
    );
    assert_eq!(
        decode(&bytes).unwrap().1,
        vec![observable_table(
            &connection,
            "sample",
            vec![0],
            vec![0],
            &[1]
        )],
    );
}

#[test]
fn storage_classes_integer_limits_real_bits_and_unknown_columns_round_trip() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sample (
                key TEXT, sequence INTEGER, payload,
                \"future text\" JSON, \"future blob\" BLOB
            )",
        )
        .unwrap();
    let values = [
        Value::Null,
        Value::Integer(i64::MIN),
        Value::Integer(i64::MAX),
        Value::Real(0.0),
        Value::Real(-0.0),
        Value::Real(f64::from_bits(1)),
        Value::Real(f64::MIN_POSITIVE),
        Value::Real(f64::MAX),
        Value::Real(f64::INFINITY),
        Value::Real(f64::NEG_INFINITY),
        Value::Real(f64::from_bits(0x3fd5_5555_5555_5555)),
        Value::Real(f64::NAN),
        Value::Text(String::new()),
        Value::Blob(Vec::new()),
        Value::Text("00001".into()),
        Value::Blob(b"00001".to_vec()),
    ];
    let raw = b"{\"duplicate\":1,\"duplicate\":2}\0\nnot-json:\xff";
    for (index, value) in values.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO sample VALUES (?1, ?2, ?3, CAST(?4 AS TEXT), ?4)",
                params![
                    format!("key-{index:02}"),
                    index as i64,
                    value,
                    raw.as_slice()
                ],
            )
            .unwrap();
    }
    snapshot(&connection);
    let rowids: Vec<i64> = (1..=values.len() as i64).collect();
    let bytes = capture(
        &connection,
        &[("sample", &["key"], &["sequence", "key"], &rowids)],
    );
    let (encoding, tables) = decode(&bytes).unwrap();
    assert_eq!(encoding, "UTF-8");
    assert_eq!(
        tables,
        vec![observable_table(
            &connection,
            "sample",
            vec![0],
            vec![1, 0],
            &rowids
        )],
    );
    assert_eq!(tables[0].rows[1][2], NativeCell::Integer(i64::MIN));
    assert_eq!(tables[0].rows[2][2], NativeCell::Integer(i64::MAX));
    assert_eq!(tables[0].rows[5][2], NativeCell::Real(1));
    assert_eq!(tables[0].rows[0][3], NativeCell::Text(raw.to_vec()));
    assert_eq!(tables[0].rows[0][4], NativeCell::Blob(raw.to_vec()));
    assert_eq!(tables[0].rows[12][2], NativeCell::Text(Vec::new()));
    assert_eq!(tables[0].rows[13][2], NativeCell::Blob(Vec::new()));
}

#[test]
fn original_text_bytes_follow_each_database_encoding() {
    for encoding in ["UTF-8", "UTF-16le", "UTF-16be"] {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "encoding", encoding)
            .unwrap();
        connection
            .execute_batch("CREATE TABLE sample (key TEXT, raw_text TEXT, raw_blob BLOB)")
            .unwrap();
        let raw: &[u8] = match encoding {
            "UTF-8" => b"\xff\0a\xc0\x80",
            "UTF-16le" => &[0x00, 0xd8, 0, 0, 0x61, 0, 0xff, 0xdc],
            _ => &[0xd8, 0x00, 0, 0, 0, 0x61, 0xdc, 0xff],
        };
        let placeholder = "x".repeat(if encoding == "UTF-8" {
            raw.len()
        } else {
            raw.len() / 2
        });
        connection
            .execute(
                "INSERT INTO sample VALUES (?1, ?2, ?3)",
                params!["A\0é𝄞", placeholder, raw],
            )
            .unwrap();
        // Set the synthetic native bytes after insertion so SQLite's text
        // conversion cannot replace invalid code units in this fixture.
        let mut native = connection
            .blob_open(DatabaseName::Main, "sample", "raw_text", 1, false)
            .unwrap();
        assert_eq!(native.len(), raw.len());
        native.write_at(raw, 0).unwrap();
        native.close().unwrap();
        snapshot(&connection);
        let bytes = capture(&connection, &[("sample", &["key"], &["key"], &[1])]);
        let (actual_encoding, tables) = decode(&bytes).unwrap();
        assert_eq!(actual_encoding, encoding);
        assert_eq!(
            tables,
            vec![observable_table(
                &connection,
                "sample",
                vec![0],
                vec![0],
                &[1]
            )],
            "{encoding}",
        );
        assert_eq!(tables[0].rows[0][1], NativeCell::Text(raw.to_vec()));
        assert_eq!(tables[0].rows[0][2], NativeCell::Blob(raw.to_vec()));
        let expected_key = match encoding {
            "UTF-8" => "A\0é𝄞".as_bytes().to_vec(),
            "UTF-16le" => "A\0é𝄞".encode_utf16().flat_map(u16::to_le_bytes).collect(),
            _ => "A\0é𝄞".encode_utf16().flat_map(u16::to_be_bytes).collect(),
        };
        assert_eq!(tables[0].rows[0][0], NativeCell::Text(expected_key));
    }
}

#[test]
fn large_cells_use_fixed_fragments_and_empty_cells_have_no_data() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sample (key TEXT, raw_text TEXT, raw_blob BLOB)")
        .unwrap();
    let raw: Vec<u8> = (0..2 * 1024 * 1024 + 17)
        .map(|index| (index % 256) as u8)
        .collect();
    connection
        .execute(
            "INSERT INTO sample VALUES ('large', CAST(?1 AS TEXT), ?1)",
            [raw.as_slice()],
        )
        .unwrap();
    connection
        .execute_batch("INSERT INTO sample VALUES ('empty', '', x'')")
        .unwrap();
    snapshot(&connection);
    let bytes = capture(&connection, &[("sample", &["key"], &["key"], &[1, 2])]);
    assert_eq!(
        decode(&bytes).unwrap().1,
        vec![observable_table(
            &connection,
            "sample",
            vec![0],
            vec![0],
            &[1, 2]
        )],
    );
    let data_lengths: Vec<usize> = bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .filter_map(
            |line| match serde_json::from_slice::<Envelope>(line).unwrap() {
                Envelope::Data { base64 } => Some(STANDARD.decode(base64).unwrap().len()),
                _ => None,
            },
        )
        .collect();
    assert_eq!(
        data_lengths
            .iter()
            .filter(|length| **length == 32_768)
            .count(),
        128
    );
    assert_eq!(
        data_lengths.iter().filter(|length| **length == 17).count(),
        2
    );
    assert_eq!(data_lengths.len(), 132);
    assert!(
        bytes
            .split_inclusive(|byte| *byte == b'\n')
            .all(|line| line.len() <= 65_536)
    );
}

#[test]
fn fragment_boundaries_preserve_final_full_and_partial_fragments() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sample (key INTEGER, raw_text TEXT, raw_blob BLOB)")
        .unwrap();
    let lengths = [0, 1, 32_767, 32_768, 32_769, 65_536];
    for (index, length) in lengths.into_iter().enumerate() {
        let bytes = vec![0xff_u8; length];
        connection
            .execute(
                "INSERT INTO sample VALUES (?1, CAST(?2 AS TEXT), ?2)",
                params![index as i64, bytes],
            )
            .unwrap();
    }
    snapshot(&connection);
    let rowids = [1, 2, 3, 4, 5, 6];
    let bytes = capture(&connection, &[("sample", &["key"], &["key"], &rowids)]);
    assert_eq!(
        decode(&bytes).unwrap().1,
        vec![observable_table(
            &connection,
            "sample",
            vec![0],
            vec![0],
            &rowids
        )],
    );
}

#[test]
fn empty_tables_and_no_selected_tables_are_complete_streams() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE empty_table (key TEXT, extra BLOB)")
        .unwrap();
    snapshot(&connection);
    assert!(decode(&capture(&connection, &[])).unwrap().1.is_empty());
    let tables = decode(&capture(
        &connection,
        &[("empty_table", &["key"], &["key"], &[])],
    ))
    .unwrap()
    .1;
    assert_eq!(
        tables,
        vec![observable_table(
            &connection,
            "empty_table",
            vec![0],
            vec![0],
            &[]
        )]
    );
}

#[test]
fn table_and_row_order_stay_with_the_caller() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE zebra (key TEXT, sequence INTEGER);
         CREATE TABLE alpha (key TEXT, unknown_column BLOB);
         INSERT INTO zebra VALUES ('first', 1), ('second', 2)",
        )
        .unwrap();
    snapshot(&connection);
    let bytes = capture(
        &connection,
        &[
            ("zebra", &["key"], &["sequence", "key"], &[2, 1]),
            ("alpha", &["key"], &["key"], &[]),
        ],
    );
    assert_eq!(
        decode(&bytes).unwrap().1,
        vec![
            observable_table(&connection, "zebra", vec![0], vec![1, 0], &[2, 1]),
            observable_table(&connection, "alpha", vec![0], vec![0], &[]),
        ]
    );
}

#[test]
fn indexes_and_hidden_rowids_do_not_change_stream_bytes() {
    let mut outputs = Vec::new();
    for (rowids, indexed) in [([1, 2], false), ([900, -40], true)] {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE sample (key TEXT, sequence INTEGER, payload TEXT)")
            .unwrap();
        for (index, rowid) in rowids.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO sample (rowid, key, sequence, payload) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        rowid,
                        format!("key-{index}"),
                        index as i64,
                        "{\"a\":1,\"a\":2}"
                    ],
                )
                .unwrap();
        }
        if indexed {
            connection
                .execute_batch("CREATE INDEX arbitrary_index ON sample (payload DESC, sequence)")
                .unwrap();
        }
        snapshot(&connection);
        outputs.push(capture(
            &connection,
            &[("sample", &["key"], &["sequence", "key"], &rowids)],
        ));
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[test]
fn quoted_identifiers_and_shadowed_rowid_aliases_are_safe() {
    let connection = Connection::open_in_memory().unwrap();
    let table = "sample\"; DROP TABLE sentinel;--";
    let column = "future\"column\n";
    connection
        .execute_batch(&format!(
            "CREATE TABLE sentinel (key); CREATE TABLE {} (rowid TEXT, _rowid_ TEXT, {} BLOB)",
            sql_name(table),
            sql_name(column),
        ))
        .unwrap();
    connection
        .execute(
            &format!(
                "INSERT INTO {} VALUES ('key', 'not a locator', x'00ff')",
                sql_name(table)
            ),
            [],
        )
        .unwrap();
    snapshot(&connection);
    let (_, tables) = decode(&capture(
        &connection,
        &[(table, &["rowid"], &["rowid"], &[1])],
    ))
    .unwrap();
    assert_eq!(tables[0].columns[2].name, column);
    assert_eq!(
        tables[0].rows[0],
        vec![
            NativeCell::Text(b"key".to_vec()),
            NativeCell::Text(b"not a locator".to_vec()),
            NativeCell::Blob(vec![0, 255])
        ]
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM sentinel", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn valid_empty_sqlite_identifiers_keep_their_exact_names() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE \"\" (\"\" TEXT, extra BLOB); INSERT INTO \"\" VALUES ('key', x'00ff')",
        )
        .unwrap();
    snapshot(&connection);
    assert_eq!(
        decode(&capture(&connection, &[("", &[""], &[""], &[1])]))
            .unwrap()
            .1,
        vec![observable_table(&connection, "", vec![0], vec![0], &[1])],
    );
}

#[test]
fn actual_read_only_connection_supports_indexed_text_and_blob_handles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.sqlite");
    let setup = Connection::open(&path).unwrap();
    setup
        .execute_batch(
            "CREATE TABLE sample (key TEXT PRIMARY KEY, payload TEXT UNIQUE, raw BLOB);
             CREATE INDEX blob_index ON sample (raw);
             INSERT INTO sample VALUES ('key', 'indexed text', x'00ff')",
        )
        .unwrap();
    drop(setup);
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    snapshot(&connection);
    assert_eq!(
        decode(&capture(
            &connection,
            &[("sample", &["key"], &["key"], &[1])]
        ))
        .unwrap()
        .1,
        vec![observable_table(
            &connection,
            "sample",
            vec![0],
            vec![0],
            &[1]
        )],
    );
}

#[test]
fn unsupported_layouts_fail_without_running_source_ddl() {
    for ddl in [
        "CREATE TABLE sample (key TEXT PRIMARY KEY) WITHOUT ROWID",
        "CREATE VIEW sample AS SELECT 'key' AS key",
        "CREATE VIRTUAL TABLE sample USING fts5(key)",
        "CREATE TABLE sample (key TEXT, derived TEXT GENERATED ALWAYS AS (key) VIRTUAL)",
        "CREATE TABLE sample (key TEXT, derived TEXT GENERATED ALWAYS AS (key) STORED)",
        "CREATE TABLE sample (key TEXT, rowid, _rowid_, oid)",
    ] {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(ddl).unwrap();
        snapshot(&connection);
        let mut writer = SqliteNativeWriter::new(
            &connection,
            |_| Ok(()),
            u64::MAX,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert!(
            matches!(
                writer.begin_table("sample", &["key"], &["key"]),
                Err(Error::UnsupportedSource)
            ),
            "{ddl}"
        );
        assert!(matches!(writer.finish(), Err(Error::InvalidSource)));
    }
}

#[test]
fn invalid_table_keys_order_and_missing_rows_fail_closed() {
    for (table, keys, order) in [
        ("absent", vec!["key"], vec!["key"]),
        ("SAMPLE", vec!["key"], vec!["key"]),
        ("sample", vec!["missing"], vec!["key"]),
        ("sample", vec!["key", "key"], vec!["key"]),
        ("sample", vec!["key"], vec!["missing"]),
        ("sample", vec!["key"], vec!["key", "key"]),
        ("sample\0suffix", vec!["key"], vec!["key"]),
    ] {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE sample (key TEXT)")
            .unwrap();
        snapshot(&connection);
        let mut writer = SqliteNativeWriter::new(
            &connection,
            |_| Ok(()),
            u64::MAX,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert!(matches!(
            writer.begin_table(table, &keys, &order),
            Err(Error::InvalidSource)
        ));
        assert!(matches!(writer.finish(), Err(Error::InvalidSource)));
    }
    for active in [false, true] {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE sample (key TEXT)")
            .unwrap();
        snapshot(&connection);
        let mut writer = SqliteNativeWriter::new(
            &connection,
            |_| Ok(()),
            u64::MAX,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        if active {
            writer.begin_table("sample", &["key"], &["key"]).unwrap();
        }
        assert!(matches!(writer.write_row(99), Err(Error::InvalidSource)));
        assert!(matches!(writer.finish(), Err(Error::InvalidSource)));
    }
}

#[test]
fn duplicate_declarations_fail_even_after_switching_tables() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE first (key); CREATE TABLE second (key)")
        .unwrap();
    snapshot(&connection);
    let mut writer = SqliteNativeWriter::new(
        &connection,
        |_| Ok(()),
        u64::MAX,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    writer.begin_table("first", &["key"], &[]).unwrap();
    writer.begin_table("second", &[], &["key"]).unwrap();
    assert!(matches!(
        writer.begin_table("first", &[], &[]),
        Err(Error::InvalidSource)
    ));
    assert!(matches!(writer.finish(), Err(Error::InvalidSource)));
}

fn try_capture(
    connection: &Connection,
    emit: impl FnMut(&[u8]) -> Result<()>,
    maximum: u64,
    deadline: Instant,
) -> Result<()> {
    let mut writer = SqliteNativeWriter::new(connection, emit, maximum, deadline)?;
    writer.begin_table("sample", &["key"], &["key"])?;
    writer.write_row(1)?;
    writer.finish()
}

#[test]
fn output_budget_is_checked_before_every_sink_call() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sample (key TEXT, payload BLOB); INSERT INTO sample VALUES ('key', zeroblob(70000))").unwrap();
    snapshot(&connection);
    let complete = capture(&connection, &[("sample", &["key"], &["key"], &[1])]);
    let mut boundaries = vec![0, 1];
    let mut offset = 0;
    for line in complete.split_inclusive(|byte| *byte == b'\n') {
        offset += line.len() as u64;
        boundaries.extend([offset - 1, offset]);
    }
    for maximum in boundaries {
        let mut written = Vec::new();
        let result = try_capture(
            &connection,
            |bytes| {
                written.extend_from_slice(bytes);
                Ok(())
            },
            maximum,
            Instant::now() + Duration::from_secs(30),
        );
        assert!(written.len() as u64 <= maximum);
        assert!(complete.starts_with(&written));
        if maximum < complete.len() as u64 {
            assert!(matches!(result, Err(Error::Limit)));
            assert!(decode(&written).is_err());
        } else {
            result.unwrap();
            assert_eq!(written, complete);
        }
    }
}

#[test]
fn sink_errors_keep_the_closed_error_and_prevent_completion() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sample (key TEXT, payload BLOB); INSERT INTO sample VALUES ('key', zeroblob(70000))").unwrap();
    snapshot(&connection);
    let complete = capture(&connection, &[("sample", &["key"], &["key"], &[1])]);
    let envelopes = complete.iter().filter(|byte| **byte == b'\n').count();
    for fail_at in 0..envelopes {
        let mut calls = 0;
        let result = try_capture(
            &connection,
            |_| {
                let index = calls;
                calls += 1;
                if index == fail_at {
                    Err(Error::Io(std::io::ErrorKind::BrokenPipe))
                } else {
                    Ok(())
                }
            },
            u64::MAX,
            Instant::now() + Duration::from_secs(30),
        );
        assert!(matches!(
            result,
            Err(Error::Io(std::io::ErrorKind::BrokenPipe))
        ));
        assert_eq!(calls, fail_at + 1);
    }
    let mut calls = 0;
    let mut writer = SqliteNativeWriter::new(
        &connection,
        |_| {
            calls += 1;
            if calls == 3 {
                Err(Error::Io(std::io::ErrorKind::StorageFull))
            } else {
                Ok(())
            }
        },
        u64::MAX,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    writer.begin_table("sample", &["key"], &[]).unwrap();
    assert!(matches!(
        writer.write_row(1),
        Err(Error::Io(std::io::ErrorKind::StorageFull))
    ));
    assert!(matches!(writer.finish(), Err(Error::InvalidSource)));
    assert_eq!(calls, 3);
}

#[test]
fn deadlines_are_checked_before_header_cells_and_fragments() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sample (key TEXT, payload BLOB); INSERT INTO sample VALUES ('key', zeroblob(70000))").unwrap();
    snapshot(&connection);
    let mut calls = 0;
    let expired = SqliteNativeWriter::new(
        &connection,
        |_| {
            calls += 1;
            Ok(())
        },
        u64::MAX,
        Instant::now() - Duration::from_secs(1),
    );
    assert!(matches!(expired, Err(Error::Limit)));
    drop(expired);
    assert_eq!(calls, 0);
    for target in [
        b"{\"kind\":\"row\"}".as_slice(),
        b"{\"kind\":\"data\",\"base64\":\"".as_slice(),
        b"{\"kind\":\"end\"}".as_slice(),
    ] {
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut seen_target = false;
        let mut after_target = 0;
        let result = try_capture(
            &connection,
            |bytes| {
                if seen_target {
                    after_target += 1;
                }
                if bytes.starts_with(target) && !seen_target {
                    seen_target = true;
                    std::thread::sleep(
                        deadline.saturating_duration_since(Instant::now())
                            + Duration::from_millis(1),
                    );
                }
                Ok(())
            },
            u64::MAX,
            deadline,
        );
        assert!(matches!(result, Err(Error::Limit)));
        assert!(seen_target);
        assert_eq!(after_target, 0);
    }
}

#[test]
fn metadata_envelope_and_total_metadata_are_bounded() {
    let connection = Connection::open_in_memory().unwrap();
    let too_long = "x".repeat(65_520);
    connection
        .execute_batch(&format!(
            "CREATE TABLE sample ({}, other)",
            sql_name(&too_long)
        ))
        .unwrap();
    snapshot(&connection);
    let mut calls = 0;
    let mut writer = SqliteNativeWriter::new(
        &connection,
        |_| {
            calls += 1;
            Ok(())
        },
        u64::MAX,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    assert!(matches!(
        writer.begin_table("sample", &[&too_long], &[&too_long]),
        Err(Error::Limit)
    ));
    drop(writer);
    assert_eq!(calls, 1);

    let connection = Connection::open_in_memory().unwrap();
    let long_column = "x".repeat(48_000);
    for index in 0..24 {
        connection
            .execute_batch(&format!(
                "CREATE TABLE t{index} ({})",
                sql_name(&long_column)
            ))
            .unwrap();
    }
    snapshot(&connection);
    let mut bytes_written = 0;
    let mut writer = SqliteNativeWriter::new(
        &connection,
        |bytes| {
            bytes_written += bytes.len();
            Ok(())
        },
        u64::MAX,
        Instant::now() + Duration::from_secs(30),
    )
    .unwrap();
    let mut limited = false;
    for index in 0..24 {
        match writer.begin_table(&format!("t{index}"), &[], &[]) {
            Ok(()) => {}
            Err(Error::Limit) => {
                limited = true;
                break;
            }
            Err(error) => panic!("unexpected error: {error}"),
        }
    }
    assert!(limited);
    assert!(matches!(writer.finish(), Err(Error::InvalidSource)));
    assert!(bytes_written <= 1024 * 1024);
}

#[test]
fn sqlite_lock_errors_are_closed_busy_values() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.sqlite");
    let blocker = Connection::open(&path).unwrap();
    blocker
        .execute_batch("CREATE TABLE sample (key); BEGIN EXCLUSIVE")
        .unwrap();
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    connection.busy_timeout(Duration::ZERO).unwrap();
    let result = SqliteNativeWriter::new(
        &connection,
        |_| Ok(()),
        u64::MAX,
        Instant::now() + Duration::from_secs(30),
    )
    .and_then(|mut writer| writer.begin_table("sample", &["key"], &["key"]));
    assert!(matches!(result, Err(Error::Busy)));
}

#[test]
fn sqlite_interruption_returns_limit_without_provider_error_text() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sample (key)")
        .unwrap();
    snapshot(&connection);
    connection.progress_handler(1, Some(|| true));
    let result = SqliteNativeWriter::new(
        &connection,
        |_| Ok(()),
        u64::MAX,
        Instant::now() + Duration::from_secs(30),
    )
    .and_then(|mut writer| writer.begin_table("sample", &["key"], &["key"]));
    let error = result.unwrap_err();
    assert!(matches!(error, Error::Limit));
    assert_eq!(error.to_string(), "desktop capture failed: Limit");
}

#[test]
fn decoder_rejects_incomplete_ambiguous_and_interleaved_records() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE sample (key, payload); INSERT INTO sample VALUES ('x', x'00ff')",
        )
        .unwrap();
    snapshot(&connection);
    let valid = String::from_utf8(capture(
        &connection,
        &[("sample", &["key"], &["key"], &[1])],
    ))
    .unwrap();
    let table = valid
        .lines()
        .find(|line| line.starts_with("{\"kind\":\"table\""))
        .unwrap();
    let variants = [
        valid.replace("{\"kind\":\"end\"}\n", ""),
        format!("{valid}{{\"kind\":\"row\"}}\n"),
        valid.replace("{\"kind\":\"row_end\"}\n", ""),
        valid.replace(
            "{\"kind\":\"row\"}\n",
            "{\"kind\":\"row\"}\n{\"kind\":\"row\"}\n",
        ),
        valid.replace(
            "{\"kind\":\"row\"}\n",
            &format!("{table}\n{{\"kind\":\"row\"}}\n"),
        ),
        valid.replace(
            "{\"kind\":\"row_end\"}",
            "{\"kind\":\"cell\",\"type\":\"null\"}\n{\"kind\":\"row_end\"}",
        ),
        valid.replace("{\"kind\":\"cell\",\"type\":\"blob\",\"bytes\":2}\n", ""),
        valid.replace("\"kind\":\"row\"", "\"kind\":\"row\",\"kind\":\"row\""),
        valid.replace("\"type\":\"blob\"", "\"type\":\"blob\",\"type\":\"blob\""),
        valid.replace("\"name\":\"key\"", "\"name\":\"key\",\"name\":\"key\""),
        valid.replace("\"version\":1", "\"version\":\"1\""),
        valid.replace("\"version\":1", "\"version\":2"),
        valid.replace("\"bytes\":2", "\"bytes\":3"),
        valid.replace("\"AP8=\"", "\"AP8\""),
        valid.replace("\"key_columns\":[0]", "\"key_columns\":[2]"),
        valid.replace("\"key_columns\":[0]", "\"key_columns\":[0,0]"),
        valid.replace("{\"kind\":\"row\"}", "{\"kind\":\"row\",\"rowid\":1}"),
        valid.replace(
            "\"type\":\"blob\",\"bytes\":2",
            "\"type\":\"blob\",\"bytes\":2,\"decimal\":null",
        ),
        valid.replace("{\"kind\":\"row\"}", "\n{\"kind\":\"row\"}"),
        valid.trim_end_matches('\n').to_owned(),
    ];
    for malformed in variants {
        assert!(decode(malformed.as_bytes()).is_err(), "{malformed}");
    }
    for malformed_cell in [
        "{\"kind\":\"cell\",\"type\":\"integer\",\"decimal\":\"01\"}",
        "{\"kind\":\"cell\",\"type\":\"integer\",\"decimal\":\"9223372036854775808\"}",
        "{\"kind\":\"cell\",\"type\":\"real\",\"bits\":\"7FF0000000000000\"}",
        "{\"kind\":\"cell\",\"type\":\"real\",\"bits\":\"0000\"}",
        "{\"kind\":\"cell\",\"type\":\"null\",\"bytes\":0}",
        "{\"kind\":\"cell\",\"type\":\"text\",\"bytes\":0}\n{\"kind\":\"data\",\"base64\":\"\"}",
    ] {
        let malformed = valid.replace(
            "{\"kind\":\"cell\",\"type\":\"blob\",\"bytes\":2}\n{\"kind\":\"data\",\"base64\":\"AP8=\"}",
            malformed_cell,
        );
        assert!(decode(malformed.as_bytes()).is_err(), "{malformed}");
    }
}
