# antiburn

![antiburn — stop hitting your token limits](docs/images/social-preview.png)

[![CI](https://github.com/antiburn/antiburn/actions/workflows/ci.yml/badge.svg)](https://github.com/antiburn/antiburn/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/antiburn/antiburn?include_prereleases&sort=semver)](https://github.com/antiburn/antiburn/releases/latest)
[![License](https://img.shields.io/github/license/antiburn/antiburn)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/antiburn/antiburn)](https://github.com/antiburn/antiburn/stargazers)
[![aislop score](https://badges.scanaislop.com/score/antiburn/antiburn.svg)](https://scanaislop.com/antiburn/antiburn)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app/)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-informational)](docs/support.md)

Local-first visibility into your AI coding-agent sessions.

antiburn discovers sessions already on your machine, analyzes transcripts
locally, and shows activity, session analysis, provider usage, and
API-equivalent cost estimates. Your session content stays on your device.

It supports Claude Code, Codex, Cursor, GitHub Copilot, Cline, OpenCode, Kiro,
Amp, Antigravity, Windsurf, and Pi. See the [support matrix](docs/support.md)
for platform limits, discovery details, and local data storage.

## Install

macOS or Linux:

```sh
curl -fsSL http://antiburn.ai/install.sh | sh
```

Windows 11 PowerShell:

```powershell
irm http://antiburn.ai/install.ps1 | iex
```

The installers verify release checksums. macOS also verifies the application
signature with Gatekeeper. Manual packages are available from the
[latest release](https://github.com/antiburn/antiburn/releases/latest).

## Development

The repository contains the Rust engine in `crates/antiburn-local` and the
Tauri desktop app in `apps/desktop`. Rust uses the toolchain in
`rust-toolchain.toml`. Desktop development also needs Node 22+, pnpm, and the
[Tauri platform dependencies](https://v2.tauri.app/start/prerequisites/).

```bash
corepack enable
pnpm install
pnpm --filter @antiburn/desktop dev
```

Run the engine checks:

```bash
cd crates/antiburn-local
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

See the [desktop guide](apps/desktop/README.md) for app commands and the
[debugging guide](docs/debugging.md) for isolated profiles and developer tools.

## Privacy

antiburn needs no account, server, or backend operated by the project. It can
contact coding-agent providers with credentials already held by your tools to
read current usage. Release builds also check GitHub Releases for updates and
can send anonymised application events. Analytics can be disabled in
Settings > Privacy and never include sessions, prompts, file paths, repository
names, or credentials. Builds from a clean checkout have no analytics endpoint.
See the complete [analytics contract](docs/analytics.md).

## Project links

- [Contributing](CONTRIBUTING.md)
- [Support](SUPPORT.md)
- [Governance](GOVERNANCE.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [MIT License](LICENSE)
- [Copyright notice](NOTICE)
- [Third-party notices](THIRD_PARTY_NOTICES)
