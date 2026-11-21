# Typed SQLite native stream

`antiburn-sqlite-native`, version `1`, preserves selected SQLite table cells
without turning their contents into a normalized session model. It is a typed
UTF-8 JSONL stream. It is not a database backup, a sequence of raw database
pages, or a JSON export of cell contents.

TEXT and BLOB cells keep their exact bytes and distinct storage classes.
Integer cells keep their signed 64-bit value. Real cells keep the bits of the
`f64` value that SQLite returns. SQLite can change an input before storing it,
for example through column affinity or conversion of NaN to NULL. This format
preserves the observable stored value, not the earlier input.

The writer does not parse JSON inside a cell. Duplicate JSON keys, malformed
JSON, unknown fields, NUL bytes, and invalid text bytes remain unchanged.
The caller selects tables and rows. A stream does not claim to contain every
table, every row, or the whole source history.

## Framing and order

Every envelope is one compact UTF-8 JSON object followed by one LF byte.
An envelope, including its LF, is at most 65,536 bytes. There are no blank lines,
duplicate object keys, or records after the final `end`.

The record grammar is:

```text
header (table (row cell* row_end)*)* end
```

Each row has exactly one cell per declared column, in column order. A TEXT or
BLOB cell includes its following `data` envelopes before the next cell.
Rows and cells cannot interleave. A table declaration applies until the next
table or `end`. Each table appears at most once. A selected empty table still
has a declaration. A stream may have no selected tables.

The writer fixes object field order as shown below. It keeps table order and
row order from the caller, and column order from SQLite. Key and order lists
use zero-based column indices. Their indices must be distinct within each
list and fall inside the column list. A column may appear in both lists.

### Header

```json
{"kind":"header","format":"antiburn-sqlite-native","version":1,"text_encoding":"UTF-8"}
```

`version` is the integer `1`, not a string. `text_encoding` is the result of
`PRAGMA main.encoding`: `UTF-8`, `UTF-16le`, or `UTF-16be`.
The JSON envelopes always use UTF-8, even when native TEXT bytes use UTF-16.

### Table declaration

```json
{"kind":"table","name":"sample","columns":[{"name":"key","declared_type":"TEXT"},{"name":"payload","declared_type":""}],"key_columns":[0],"order_columns":[0]}
```

`name`, column names, and declared types come from the source schema.
An empty declared type stays empty. Every ordinary column is present, including
columns unknown to the provider.

The declaration has no create SQL, indexes, SQLite column IDs, constraints,
defaults, hidden-column flags, or physical row IDs. It describes the selected
cell stream, not a schema to execute or recreate.

### Rows and scalar cells

```json
{"kind":"row"}
{"kind":"cell","type":"null"}
{"kind":"cell","type":"integer","decimal":"-9223372036854775808"}
{"kind":"cell","type":"real","bits":"3ff0000000000000"}
{"kind":"row_end"}
```

These cell examples show separate cell forms. The table declaration determines
how many cells a particular row needs.

An integer uses its canonical signed decimal string, from
`-9223372036854775808` through `9223372036854775807`. A real uses exactly
16 lowercase hexadecimal digits for `f64::to_bits()`. The hex value describes
the numeric bit pattern and does not depend on host byte order. JSON numbers
do not carry integer or real cell values.

A row marker has no key or row ID. Cell position identifies the column.
Declared key columns contain the original cells like any other columns.

### TEXT and BLOB cells

```json
{"kind":"cell","type":"text","bytes":5}
{"kind":"data","base64":"YQD/Ygo="}
{"kind":"cell","type":"blob","bytes":0}
```

`bytes` is an unsigned 64-bit byte count. A TEXT cell contains native bytes in
the database's declared encoding. It can include invalid encoded text.
A BLOB cell contains raw bytes with no text encoding.

Data uses padded standard Base64. Every non-final fragment decodes to exactly
32,768 bytes. The final fragment decodes to the remaining 1 through 32,768
bytes. A zero-length cell has no data envelopes. There are no offsets:
the declared byte count and preceding fragments determine each position.
Readers must reject missing, extra, short non-final, and oversized fragments.

### End

```json
{"kind":"end"}
```

Readers must require this record. EOF, a partial row, or a partial cell does
not establish a complete stream.

## Writer and provider boundary

The internal `SqliteNativeWriter` accepts an already-open SQLite connection,
a fallible byte sink, an output byte limit, and a deadline. Construction emits
the header. `begin_table(name, key_columns, order_columns)` declares a table.
Table and column names must exactly match the source. `write_row(rowid)`
emits a row from the active table. `finish()` emits `end`.
Run this synchronous writer on a bounded blocking worker, never on a UI or
async runtime thread.

The provider owns authorization, read-only connection flags, safe SQLite
settings, a snapshot transaction, table selection, and deterministic logical
row order. It must keep that snapshot open for the whole write. The writer
does not open a source, choose a provider, start a transaction, attach a
database, load an extension, or execute source DDL.

Key and order declarations do not validate row uniqueness, nullability, row
ownership, or sorted order. The provider must prove these properties before
using declared keys as native identity.

The physical row ID is only an internal locator. Changing an unexposed row ID
or an index does not change output when the selected values and logical row
order stay the same. An explicitly declared integer primary-key column remains
a cell, so changing that value does change output.

The writer supports ordinary `main` rowid tables only. It rejects views,
virtual or shadow tables, `WITHOUT ROWID`, generated columns, and tables that
shadow all three rowid aliases: `_rowid_`, `rowid`, and `oid`.
It has no whole-cell fallback for unsupported layouts.

TEXT and BLOB reads use SQLite's standard incremental BLOB handle with
read-only flags (`0`), including for indexed TEXT. The writer never reads a
whole native TEXT or BLOB into a Rust `String` or `Vec`. It reads at most
32 KiB of native cell bytes per fragment. Scalar queries return TEXT/BLOB
type markers instead of the native cell contents.

The sink receives one complete envelope per successful call. The writer checks
the output budget before calling it. Header and table declarations together
are limited to 1 MiB; each declaration also fits the 64 KiB envelope limit.
The writer checks the deadline before metadata reads, each cell, and each
fragment, and before and after each sink call. Connection busy timeouts and
SQLite progress handling remain caller responsibilities. A deadline does not
interrupt a blocked sink.

## Errors and recovery

The writer returns closed crate errors. It does not return raw SQL, SQLite
messages, or cell values in errors.

| Error | Meaning |
| --- | --- |
| `UnsupportedSource` | The selected source uses an unsupported table layout or encoding. |
| `InvalidSource` | A table, column, row locator, declaration, or SQLite source fact is invalid or missing. |
| `Busy` | SQLite reports a locked or busy database. |
| `Limit` | The output, metadata, envelope, or execution bound is reached. SQLite interruption, size, and memory-limit errors also use this value. |
| Sink error | The sink's existing closed crate error passes through unchanged. |

`UnsupportedFormat` is separate. It identifies an unsupported saved capture
format, not an unsupported SQLite source layout.

After any error, discard the writer and treat all emitted bytes as incomplete.
After a table or row operation fails, `finish()` refuses to emit `end`.
The writer does not repair cells, retry reads, or publish partial output as a
complete capture. The caller owns private partial-output handling and retry
identity under the [native capture contract](capture.md). Correct an access
problem or unsupported selection before taking a new authorized snapshot.
Do not edit native cells or saved format versions to force acceptance.
