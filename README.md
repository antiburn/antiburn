# antiburn

Local-first visibility into your AI coding-agent sessions.

antiburn discovers the coding-agent sessions already on your machine — Claude Code,
Codex, Cursor, GitHub Copilot, Cline, OpenCode, Kiro, Amp, Pi, Antigravity, and
Windsurf — analyzes the transcripts locally, and shows you activity, session
analytics, and API-equivalent cost estimates. Everything runs on your device.

**Network boundary:** the engine performs no network or socket I/O — discovery reads
documented files, read-only databases, and bounded WSL paths only. The desktop app
(coming to this repository) is useful fully offline; its only internet exceptions are
GitHub-hosted updates and separately consented, default-off anonymous analytics.

## Repository layout

```text
crates/antiburn-local/   The engine: discovery, session analysis, repository
                         identity, pricing, and local persistence contracts.
                         Standalone workspace with its own lockfile.
apps/desktop/            The desktop application: a Tauri 2 menu-bar shell
                         (React 19 + TypeScript) over the engine. Its Rust
                         crate is a standalone workspace of its own.
docs/oss/                Approved source-boundary manifests; the engine's
                         mechanical boundary tests validate against them.
```

## Build and test

### Engine

Requires Rust (see `rust-toolchain.toml`).

```bash
cd crates/antiburn-local
cargo build
cargo test
```

The test suite includes mechanical source-boundary checks (`tests/boundary.rs`)
that enforce the network-free, local-only contract: prohibited concepts,
network-capable dependencies, and manifest integrity all fail the build.

### Desktop application

Additionally requires Node 22+ with pnpm (`corepack enable`) and the Tauri
platform dependencies. See [`apps/desktop/README.md`](apps/desktop/README.md).

```bash
pnpm install
pnpm --filter @antiburn/desktop dev
```

## Provenance

This repository starts from a fresh Git root containing only the approved public
subset of the original private implementation, admitted under the source manifests
in `docs/oss/`. See `NOTICE` and `LICENSE` (MPL-2.0).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Contributions require a
Developer Certificate of Origin sign-off (`git commit -s`); there is no CLA.

## Security

See [SECURITY.md](SECURITY.md) for how to report vulnerabilities privately.
