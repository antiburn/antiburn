# antiburn

> Tiny, fast, local burn checks for all your coding sessions.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/3369144d-61b0-4b94-8373-41f2541cba95" />
  <img width="100%" alt="antiburn: the popover with live limit meters and today's sessions, a session's context chart with a compaction, and its cost and tools breakdowns" src="https://github.com/user-attachments/assets/d1c1404e-4e6e-4ef3-8dbd-726150888e3d" />
</picture>

[![License](https://img.shields.io/github/license/antiburn/antiburn)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-informational)](docs/support.md)
[![Release](https://img.shields.io/github/v/release/antiburn/antiburn?filter=antiburn-v*)](https://github.com/antiburn/antiburn/releases/latest)
[![CI](https://github.com/antiburn/antiburn/actions/workflows/ci.yml/badge.svg)](https://github.com/antiburn/antiburn/actions/workflows/ci.yml)
[![GitHub stars](https://img.shields.io/github/stars/antiburn/antiburn)](https://github.com/antiburn/antiburn/stargazers)
[![aislop score](https://badges.scanaislop.com/score/antiburn/antiburn.svg)](https://scanaislop.com/antiburn/antiburn)
[![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://tauri.app/)
[![Slack](https://img.shields.io/badge/Slack-join-4A154B?logo=slack&logoColor=white)](https://antiburn.ai/slack)

A little free desktop app to check your sessions for the most common causes of token burn - sessions that go too deep, subagents that go too hard, skills and MCPs that go unused, etc etc etc.

antiburn supports Claude Code, Codex, Cursor, GitHub Copilot, Cline, OpenCode, Kiro, Amp, Antigravity, Windsurf, and Pi. See the [support matrix](docs/support.md) for platform limits, discovery details, and local data storage.

## Checks

- Excess cache rehydration - cache writes are expensive; let's all work out how to avoid too many of them.
- Fast mode overuse - fast mode is great if you're not close to limit, but be careful if you are.
- Model overthinking - I know `xhigh` and `ultra` sound cool but they're usually better avoided.
- Old model usage - worth checking if you're still pinned to old models, especially in subagents.
- Overpowered subagents - using subagents on premium models is generally a bad idea.
- Session overdepth - compaction works now, friends don't let friends have 950k context windows.
- Unused built-in tools - Claude (especially) has a bunch of heavy built-in tools that you should probably disable.
- Unused MCP servers - MCPs are usually situational, just turn them on when you need them.
- Unused skills - most of us have skills installed that cost tokens every session, but that we never use any more.

## Install

macOS or Linux:

```sh
curl -fsSL https://antiburn.ai/install.sh | sh
```

Windows 11 PowerShell:

```powershell
irm https://antiburn.ai/install.ps1 | iex
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

antiburn needs no account, server, or backend operated by the project. It can contact coding-agent providers with credentials already held by your tools to read current usage. Release builds also check GitHub Releases for updates.

antiburn has analytics that we use to improve the app, but analytics events never include sessions, prompts, file paths, repository names, or credentials. Builds from a clean checkout have no analytics endpoint. Point your coding agent at this repo to check exactly what data leaves the device, if you're worried. See the complete [analytics contract](docs/analytics.md).

## Community

Questions, fixes, what's burning: join the
[antiburn Slack](https://antiburn.ai/slack). Bugs and feature requests go in
[GitHub issues](https://github.com/antiburn/antiburn/issues).

## Project links

- [Contributing](CONTRIBUTING.md)
- [Support](SUPPORT.md)
- [Governance](GOVERNANCE.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [MIT License](LICENSE)
- [Copyright notice](NOTICE)
- [Third-party notices](THIRD_PARTY_NOTICES)
