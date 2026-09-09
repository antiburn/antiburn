<h1 align="center">🔥 antiburn</h1>

<p align="center">
  <strong>Find what burns through your coding-agent tokens.</strong><br />
  A free, local desktop app for your menu bar or system tray.
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#first-use">First use</a> ·
  <a href="#checks">Checks</a> ·
  <a href="#privacy">Privacy</a> ·
  <a href="#help">Help</a>
</p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://github.com/user-attachments/assets/3369144d-61b0-4b94-8373-41f2541cba95" />
  <img width="100%" alt="antiburn: the popover with live limit meters and today's sessions, a session's context chart with a compaction, and its cost and tools breakdowns" src="https://github.com/user-attachments/assets/d1c1404e-4e6e-4ef3-8dbd-726150888e3d" />
</picture>

<p align="center">
  <a href="https://github.com/antiburn/antiburn/releases/latest"><img alt="Release" src="https://img.shields.io/github/v/release/antiburn/antiburn?filter=antiburn-v*" /></a>
  <a href="https://github.com/antiburn/antiburn/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/antiburn/antiburn/actions/workflows/ci.yml/badge.svg" /></a>
  <a href="docs/support.md"><img alt="Platforms: macOS, Windows, Linux" src="https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-informational" /></a>
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/github/license/antiburn/antiburn" /></a>
</p>

antiburn reads the session data your coding agents already save locally. Review
context growth, token and cost breakdowns, and checks for common causes of token
burn—from long-running sessions to unused tools and expensive subagents.

It supports Claude Code, Codex, Cursor, GitHub Copilot, Cline, OpenCode, Kiro, Amp,
Antigravity, Windsurf, and Pi. **Check coverage varies by agent and source format;
it is broadest for Claude Code and Codex.** See [agent and platform support](docs/support.md)
and [check coverage](docs/check-coverage.md#coverage-matrix). The support guide also
explains discovery paths and [local data storage](docs/support.md).

## Install

**Requirements:** macOS 13+ (Apple silicon or Intel), Windows 11 (x86-64), or a
mainstream Linux desktop (x86-64) with a system tray or AppIndicator host.
See [platform details](docs/support.md#platforms) for Linux display-server limits.

**macOS or Linux**

```sh
curl -fsSL https://antiburn.ai/install.sh | sh
```

**Windows 11 — PowerShell**

```powershell
irm https://antiburn.ai/install.ps1 | iex
```

The installers verify release checksums. macOS also verifies the application
signature with Gatekeeper. Prefer a manual install? Download a package from the
[latest release](https://github.com/antiburn/antiburn/releases/latest).

## First use

1. Open antiburn and follow onboarding to review detected agents and session sources.
2. Finish setup, then open antiburn from your **menu bar or system tray**.
3. Open a discovered session to review the available analysis and check results.

Missing session data can limit what antiburn can assess. **No finding does not
necessarily mean a session is clean.** The [coverage guide](docs/check-coverage.md#status-rules)
explains the evidence required for findings and clean results.

## Checks

| Check                 | What to review                                       |
| --------------------- | ---------------------------------------------------- |
| Session overdepth     | Sessions whose context has grown too large.          |
| Model overthinking    | High reasoning settings that may not suit the task.  |
| Overpowered subagents | Subagent work assigned to premium models.            |
| Unused MCP servers    | Configured MCP tools that the session does not use.  |
| Unused built-in tools | Built-in tools that the session does not use.        |
| Unused skills         | Skills loaded into the session but not used.         |
| Old model usage       | Sessions or subagents still using older models.      |
| Fast mode overuse     | Fast-mode usage worth reviewing against your limits. |
| Cache churn           | Repeated cache writes and rehydration.               |

These are prompts to review your setup, not a claim that every flagged choice is
wrong. Read the [check definitions and evidence boundaries](docs/check-coverage.md#evidence-boundaries)
for what antiburn can establish from each source.

## Privacy

**Your session content stays local. No antiburn account is required.**

- **Provider connections:** antiburn can use your existing provider credentials
  to retrieve usage and plan limits. Background provider polling is enabled by
  default. It also makes pricing-catalog requests to models.dev and checks
  GitHub Releases for updates. See the [network details](docs/support.md#network).
- **Product analytics:** official builds start with analytics enabled. Events
  exclude transcripts, prompts, file paths, repository names, and credentials,
  but include coarse usage and diagnostic categories. The
  [analytics contract](docs/analytics.md) documents the fields, identifiers,
  endpoint metadata, and retention limits.
- **Your control:** turn analytics off in **Settings → Privacy**. Builds from a
  clean checkout have no analytics endpoint and send no analytics.

The code is open source. You can audit the
[analytics implementation](apps/desktop/src-tauri/src/analytics) and the documented
network behavior yourself—or ask your coding agent to review them.

## Help

- **No sessions appearing?** Check your agent's discovery paths and supported
  source formats in the [support guide](docs/support.md#agents).
- **No tray icon on Linux?** Your desktop needs a system tray or AppIndicator
  host. Check the [platform requirements](docs/support.md#platforms).
- **Something else?** See [support](SUPPORT.md) and the
  [debugging guide](docs/debugging.md), or ask in [Slack](https://antiburn.ai/slack).

Report bugs and request features in
[GitHub issues](https://github.com/antiburn/antiburn/issues). For bugs, include your
OS, antiburn version, agent, and reproduction steps. Do not post credentials or
private session content. Report security issues through the [security policy](SECURITY.md).

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

See the [desktop guide](apps/desktop/README.md) for app commands, the
[debugging guide](docs/debugging.md) for isolated profiles and developer tools,
and [Contributing](CONTRIBUTING.md) for the full validation and contribution requirements.

## License and project policies

[MIT License](LICENSE) · [Copyright notice](NOTICE) ·
[Third-party notices](THIRD_PARTY_NOTICES) · [Governance](GOVERNANCE.md) ·
[Code of Conduct](CODE_OF_CONDUCT.md)
