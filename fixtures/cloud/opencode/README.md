# OpenCode native capture fixtures

These files contain authored synthetic values only. They contain no real
sessions, credentials, machine captures, or private service fixtures.

`earlier.sql` defines the minimum supported V2 session/message capability.
`current.sql` uses the public OpenCode `v2.3.0` table shapes at commit
`a6b15d33daf6ab7d35569f6d711fdb630b9f04e7`, with synthetic session-owned values.
The fixture is a capture-selection input, not a complete replay of every
producer event schema.

The current input includes:

- Independent main, child, detached-child, and fork identities.
- Inline and split assistant content, plus opaque unknown source.
- Current and removed instruction entries, initial/current instruction state,
  retired event references, and a fork-owned instruction baseline.
- Media referenced by current rows, split/inline tool content, retained events,
  and queued inputs. Text MIME types must not cause source omission.
- Parent-owned job output, errors, and child references.
- Unrelated credentials, schedules, child content, jobs, and shared values that
  must not enter the selected main capture.

Tests construct SQLite databases from these SQL files in private temporary
directories. They never open installed OpenCode stores. Provider tests check
selection, snapshots, admission, and saved retries. Separate generic SQLite
format tests own the typed-value decoder oracle.

`reference/main.capture.json` and its `main.native-rows.chunk-*.gz` and
`main.capture-metadata.chunk-*.gz` files come from real spool capture of
`current.sql` session `ses_main`. The database filename is `opencode.db`, the
scope is `synthetic-root`, and the capture ID is `fixture-opencode-main`.
Account and destination are `synthetic-account` and `synthetic-destination`.
Temporary directory paths do not enter these references. Both wire envelopes use
numeric capture version `1`; SQLite native format version is inside its header.

The reference retains selected opaque values and native blob data. It excludes
other session bodies and unrelated global values under the source ownership
rules. Consumers must decode the typed stream before interpreting its content;
the compressed body is not a database file or normalized session document.

`manifest.json` pins input, documentation, and actual wire-reference bytes.
Tests capture the same source in two private temporary roots and compare every
manifest field and saved gzip body with the references. Regenerate only for an
intentional byte-contract change:

```sh
cargo run --manifest-path crates/antiburn-cloud-capture/Cargo.toml --features cloud --locked --example write_fixture
```

This command regenerates both public capture packages. See the
[OpenCode capture contract](../../../docs/cloud/opencode.md) and
[SQLite native format](../../../docs/cloud/sqlite-native.md).
