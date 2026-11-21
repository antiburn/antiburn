# SQLite WAL initialization backport

This directory vendors the complete crates.io `libsqlite3-sys 0.30.1` source
(MIT; see LICENSE), excluding Cargo cache metadata. SQLite is public domain;
SQLCipher retains its license in `sqlcipher/LICENSE`.

The only upstream source edits are in `sqlite3/sqlite3.c` and
`sqlcipher/sqlite3.c`: the exact `sqlite3WalReadFrame` change from
https://github.com/sqlite/sqlite/commit/e5bf119821f25be1d7f10ef0a2edbe175289166e.
It zeroes the unread suffix before reading a shorter WAL frame. The normal
read length, error result, ABI, bindings, build flags, and version remain unchanged.
`wal-regression.c` and this document are local additions.

Addresses RUSTSEC-2026-0459 / CVE-2026-119375 in these bundled sources only.
This is not a claim that SQLite 3.46.0 has every fix from SQLite 3.54.0.
The SQLCipher copy receives the same fix but its encrypted build is not covered
by the standard SQLite regression executable.

## Regression

From this directory on macOS or Linux:

```sh
cc -O0 -g wal-regression.c -o /tmp/wal-regression -lpthread -ldl -lm
/tmp/wal-regression
```

The test compiles the actual bundled amalgamation and fills the output with
poison bytes. It checks smaller, equal, and larger output sizes, including the
65536-byte encoding, read offsets, error propagation, and untouched guard bytes.
It must fail with the upstream change reversed. The checks remain active even when SQLite defines `NDEBUG`.

## Cargo and consumers

All five SQLite workspace roots (engine, checks-store, remote helper, CLI, and
desktop) patch crates.io to this directory. Cargo ignores
patches in dependencies. A downstream Git or path consumer MUST repeat the
patch in its own workspace root; merely updating `antiburn-local` is insufficient.
The engine is `publish = false`; there is no crates.io engine release. Its source
archive includes this directory and its standalone patch without outside paths.

For a Git consumer, use the same full reviewed Antiburn commit SHA in both entries:

```toml
[dependencies]
antiburn-local = { git = "https://github.com/antiburn/antiburn", rev = "<full-sha>" }

[patch.crates-io]
libsqlite3-sys = { git = "https://github.com/antiburn/antiburn", rev = "<full-sha>" }
```

For an extracted engine archive used as a path dependency:

```toml
[patch.crates-io]
libsqlite3-sys = { path = "path/to/antiburn-local/vendor/libsqlite3-sys" }
```

Verify `cargo tree -i libsqlite3-sys` selects this source and exactly one
`links = "sqlite3"` package. `rusqlite 0.32.1` and SQLx 0.8 can share version
0.30.1. Recheck after dependency updates; a newer matching registry version
can displace a patch. A consumer's lockfile alone does not distribute this fix.

`LIBSQLITE3_SYS_USE_PKG_CONFIG`, `sqlcipher` without `bundled-sqlcipher`,
`loadable_extension`, and `in_gecko` can bypass the patched bundled source.
Those configurations must independently supply a fixed SQLite. They are not
Antiburn's supported distribution build configuration.

Version-based advisory scanners may continue reporting this advisory. Some
source-aware tools omit local patches from registry advisory matching; a clean
SQLite result does not prove this backport is correct. No
advisory ignore is added and no upstream version is fabricated. The source
backport and regression evidence must accompany any security review.
Remove this vendor and all five root patches once SQLx and rusqlite can resolve a
shared upstream fixed binding version. Keep the root patches consistent until then.

## Provenance

Original crates.io archive SHA-256:

```text
2e99fb7a497b1e3339bc746195567ed8d3e24945ecd636e3619d20b9de9e9149
```

The external consumer fixture in `../../tests/sqlite-compat` builds and queries
SQLite through both rusqlite 0.32.1 and SQLx 0.8.6. Run it from the engine root:

```sh
cargo run --manifest-path tests/sqlite-compat/Cargo.toml --locked
cargo tree --manifest-path tests/sqlite-compat/Cargo.toml -i libsqlite3-sys --locked
```

The same fixture is included in the engine source archive. Its root patch is
intentional: deleting it selects the vulnerable registry source.

## Validation

The engine CI matrix runs the C regression and external SQLx consumer on
Linux, macOS, and Windows. The standard Rust checks also cover every affected
workspace; the remote helper runs on Linux only. Dependency checks include all
five SQLite roots. The third-party notices generator retains the vendored
binding's MIT inventory entry and license text even when Cargo reports no
registry source.

The 2026-11-13 Apple Silicon macOS integration check verifies the original
crate archive checksum and confirms that only the two documented amalgamation
hunks differ from upstream. The patched C regression passes all 50 cases;
the original-source negative control fails on the nonzero unread suffix.
The source-tree, extracted source archive, and same-revision Git consumer
fixtures build and query through both SQL clients. Cargo metadata selects one
`links = "sqlite3"` package from each intended patched source.
All five roots pass cargo-deny advisories, bans, and licenses without adding
advisory ignores. These source-aware results do not independently prove the
backport; the source comparison and positive/negative regression provide that
evidence.

Linux and Windows executions and packaged application bundles remain CI or
release checks, not local validation claims.
