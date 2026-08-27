# antiburn-local

`antiburn-local` is the local engine behind the antiburn desktop application.
It discovers coding-agent sessions, analyzes transcripts, resolves repository
identity, calculates API-equivalent cost estimates, and owns versioned local
data contracts.

The crate has no project service, account, telemetry, or private dependency. It
reads local agent data and provider-owned stores without modifying them.

## Use

The crate is released as a source archive and is not published to crates.io.
Consumers pin the full commit SHA for an `antiburn-local-v*` release tag. See
the [release guide](../../docs/runbooks/release.md#how-consumers-pin-the-engine).

Public modules and compatibility changes are documented in the
[crate changelog](CHANGELOG.md). Run the standalone checks with:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

The project is licensed under the [MIT License](../../LICENSE).
