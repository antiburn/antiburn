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
[![Slack](https://img.shields.io/badge/Slack-join-4A154B?logo=slack&logoColor=white)](https://antiburn.com/slack)

A little free desktop app to check your sessions for the most common causes of token burn - sessions that go too deep, subagents that go too hard, skills and MCPs that go unused, etc etc etc.

antiburn supports Claude Code, Codex, Cursor, GitHub Copilot, Cline, OpenCode, Kiro, Amp, Antigravity, Devin (Windsurf), Pi, and Oh My Pi. See [supported agents](https://antiburn.com/docs/agents/supported-agents/) for what antiburn reads from each one, and the [support matrix](docs/support.md) for platform limits, discovery details, and local data storage.

## Agent plugin

The [antiburn plugin page](plugins/README.md) describes the unified Claude Code
plugin, its local development build, and the current distribution status.
The plugin works independently of the desktop app.

## Checks

Each check has a docs page explaining what it measures and how to fix it. The [config guides](https://antiburn.com/guides/) cover the Claude Code, Codex, and Pi settings behind most of them.

- [Excess cache rehydration](https://antiburn.com/docs/findings/excess-cache-rehydration/) - cache writes are expensive; let's all work out how to avoid too many of them.
- [Fast mode overuse](https://antiburn.com/docs/findings/fast-mode-overuse/) - fast mode is great if you're not close to limit, but be careful if you are.
- [Model overthinking](https://antiburn.com/docs/findings/model-overthinking/) - I know `xhigh` and `ultra` sound cool but they're usually better avoided.
- [Old model usage](https://antiburn.com/docs/findings/old-model-usage/) - worth checking if you're still pinned to old models, especially in subagents.
- [Overpowered subagents](https://antiburn.com/docs/findings/overpowered-subagents/) - using subagents on premium models is generally a bad idea.
- [Session overdepth](https://antiburn.com/docs/findings/session-overdepth/) - compaction works now, friends don't let friends have 950k context windows.
- [Unused built-in tools](https://antiburn.com/docs/findings/unused-built-in-tools/) - Claude (especially) has a bunch of heavy built-in tools that you should probably disable.
- [Unused MCP servers](https://antiburn.com/docs/findings/unused-mcp-servers/) - MCPs are usually situational, just turn them on when you need them.
- [Unused skills](https://antiburn.com/docs/findings/unused-skills/) - most of us have skills installed that cost tokens every session, but that we never use any more.

### Write your own checks with YAML

[Burn Checks](docs/burn-check/README.md) let you define what to check in `.yml`
files. Put a file in your project's `.antiburn/checks/` folder or use
`~/.antiburn/checks/` for checks across projects. antiburn discovers the files,
but does not run them until a configured connection is selected and **Run checks**
is on. Check results appear in Checks. The repository includes five examples for
exploration, task scope, instructions, skill guidance, and fast-mode advice.

The `v1-alpha` runtime supports public user and assistant messages, tool calls
and results, and response-cycle endings that recorded evidence can prove. A
check can also run when antiburn attaches to a session or observes 120 seconds
without source activity. Available events depend on the agent's recorded
evidence; the app does not invent missing events. A check can set status with
local rules or ask a compatible model a bounded question. See the
[supported subset](docs/burn-check/authoring.md#supported-subset) for exact events
and limits.

Open **Settings → Checks → Model-based checks**, choose **Add connection**, and
configure a supported System One connection: Jev, an Ollama endpoint that supports
System One, Cloudflare, or Custom. Ordinary chat endpoints are not supported.
The first connection becomes selected without enabling checks. Select **Use**
for another saved connection, then enable **Run checks**. An optional **Test
connection** sends synthetic input. Model requests can incur provider charges.
Selected public text, tool results, and referenced current files can contain
private data and can leave your device. Private thinking stays excluded.

Alpha is experimental and can introduce breaking changes. It does not support
message actions, effective session skill inventories, `repo.branch`, or proof
of which instructions were active in the past. Beta and stable scope are not
set. YAML checks do not provide Auto Fix, verification watches, or savings
estimates. Read the [specification](docs/burn-check/specification.md) and its
implementation limits before writing a check.

YAML checks replace four fixed model-based detectors. Their old findings do not
become YAML results. See [upgrade notes](docs/burn-check/README.md#upgrade-from-fixed-model-based-checks)
for the history and settings changes.

To help your coding agent author checks, install the
[Burn Check authoring skill](.agents/skills/antiburn-checks/SKILL.md) with
the [Skills CLI](https://skills.sh/docs):

```sh
npx skills add antiburn/antiburn --skill antiburn-checks
```

## Install

Full walkthrough: [installation](https://antiburn.com/docs/getting-started/installation/) and [updating](https://antiburn.com/docs/getting-started/updating/).

macOS 13 or later, with Homebrew (Apple silicon and Intel):

```sh
brew install --cask antiburn/tap/antiburn
```

If you already installed antiburn from a DMG or curl, quit it and move the old
`/Applications/antiburn.app` to the Trash before installing with Homebrew. Keep
the local data folder to preserve your settings and history. See
[installation and updates](docs/support.md#installation-and-updates).

Shell installer for macOS or Linux:

```sh
curl -fsSL https://antiburn.com/install.sh | sh
```

Windows 11 PowerShell (x64 and ARM64):

```powershell
irm https://antiburn.com/install.ps1 | iex
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
cargo nextest run
```

See the [desktop guide](apps/desktop/README.md) for app commands and the
[debugging guide](docs/debugging.md) for isolated profiles and developer tools.

## Privacy

antiburn uses no account, server, or backend. It hits agent provider APIs from your machine, using the same methods your harnesses already do.

antiburn has analytics that we use to improve the app, but analytics events never include anything you care about: sessions, prompts, file paths, repository names, credentials, etc. You can turn analytics off, and builds from a clean checkout have no analytics endpoint. See [privacy and data](https://antiburn.com/docs/privacy/privacy-and-data/) and the complete [analytics contract](docs/analytics.md).

Open source, so if you're worried, point your coding agent at this repo to audit exactly what data leaves the device.

## Community

Questions, fixes, what's burning: join the [antiburn Slack](https://antiburn.com/slack).

Bugs and feature requests go in [GitHub issues](https://github.com/antiburn/antiburn/issues).

## Project links

- [Docs](https://antiburn.com/docs/)
- [Config guides](https://antiburn.com/guides/)
- [Resources](https://antiburn.com/resources/)
- [Release notes](https://antiburn.com/release-notes/)
- [Contributing](CONTRIBUTING.md)
- [Support](SUPPORT.md)
- [Governance](GOVERNANCE.md)
- [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [MIT License](LICENSE)
- [Copyright notice](NOTICE)
- [Third-party notices](THIRD_PARTY_NOTICES)
