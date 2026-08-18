# antiburn

Local-first visibility into your AI coding-agent sessions.

antiburn discovers the coding-agent sessions already on your machine, analyzes the
transcripts locally, and shows you activity, session analytics, and API-equivalent
cost estimates. Everything runs on your device.

**Supported agents.** Claude Code, Codex, Cursor, GitHub Copilot, Cline, OpenCode,
Kiro, and Amp are discovered from their documented local files on every supported
platform. Three carry qualifications:

- **Antigravity** and **Windsurf** read only documented local files: their live
  language-server APIs aren't read, so a session that exists only in that live
  state won't appear.
- **Pi** is not supported on Windows.
- **WSL** discovery covers **Claude Code, Codex, and OpenCode** only. Other agents are
  found in the native environment only.

Supported platforms are macOS 13 or later, Windows 11, and mainstream x86-64 Linux
desktops. See [`docs/support.md`](docs/support.md) for the full matrix and for what
antiburn stores.

**Local boundary:** antiburn needs no connection to any service of ours — no antiburn
account, server, or backend, ever. Everything runs on this machine, as you. The
connections it makes beyond that are yours: it can read your provider's own current
usage figures with your own credentials — traffic between this machine and a provider
you already use, never us. The one call antiburn makes to a service of ours is the update check, against
GitHub Releases, and the app never depends on it. There is no analytics or telemetry
of any kind in this application, and antiburn hands your data to no one who doesn't
already have it.

**Resource boundary:** antiburn is a background utility, so CPU, memory, and disk I/O
are product constraints. Work must be lazy, bounded, and no more frequent or
memory-intensive than the visible feature requires. It should not materially load a
reader's machine merely because it is running.

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
docs/support.md          The v1 platform and agent support matrix, and what
                         antiburn stores about your sessions.
docs/deviations.md       Every deliberate difference from the ratified feature
                         matrix, with its reason and revisit milestone.
docs/macos-folder-access.md
                         How antiburn asks for the folders macOS guards, and
                         why discovery is shaped around never surprising you
                         with a permission dialog.
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
that enforce the engine's local contract: no dependency on any service of ours,
prohibited concepts (telemetry, commercial identifiers), and manifest integrity
all fail the build.

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
