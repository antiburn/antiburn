# Antiburn CLI

Desktop distribution builds package the native `antiburn-cli` executable and prepare its durable per-user runtime automatically. See [installation, ownership and acceptance](../../docs/cli-installation.md). Probe `antiburn-cli capabilities --json` without opening any database. The existing `antiburn` command remains the desktop launcher.

The CLI gives agent harnesses a small JSON interface for Antiburn check updates.

**Current status:** `antiburn-cli checks <session_id>` reads Antiburn's local SQLite database in read-only mode, derives six evidence-based checks with `antiburn-local`, synthesizes a bounded priced-session outlier from cached analysis, and returns only new or changed problematic findings. It omits raw evidence from stdout and tracks per-session changes in a separate local state file.

The CLI starts a Tokio current-thread runtime. Stdout and stderr use Tokio async I/O. The SQLite lookup runs in `spawn_blocking` because rusqlite is synchronous. The low-volume JSON Lines tracing subscriber uses its synchronous file writer for diagnostics. Argument parsing, validation, and JSON serialization remain synchronous.

## Build and run

From this directory:

```sh
cargo build
cargo nextest run
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

The executable is `target/debug/antiburn-cli`:

```sh
./target/debug/antiburn-cli checks example-session-id
```

The successful response is one JSON object on stdout. The CLI returns only new or changed problematic checks. It omits raw evidence from the response:

```json
{
  "session_id": "example-session-id",
  "identity": {
    "environment_key": "native",
    "agent": "pi",
    "session_id": "example-session-id"
  },
  "freshness_evaluated": false,
  "checks": [
    {
      "source": "session_evidence_badge",
      "check_id": "sessions_over_depth",
      "status": "finding",
      "result": {"outcome": "finding"}
    }
  ]
}
```

`freshness_evaluated` is always `false`. The CLI does not apply the desktop's full evidence-currentness rules. A returned evidence-derived finding reflects the stored evidence snapshot; do not describe it as current. The CLI evaluates clean states internally to detect resolution; missing, not-assessed, and incomplete checks preserve the last assessed fingerprint, but it never returns them. For persisted rows, `result_matches_input_revision` is only one part of currentness; the CLI returns a finding only when a completed result has findings. Malformed stored JSON or a missing required table returns an error on stderr and a non-zero exit code.

Every returned item in `checks` names its source. Persisted findings come from `burn_check_assessment`. Evidence-derived findings come from the public `antiburn_local::insights::session_badges_with_selection` API, which uses the same default catalogs as the desktop. It derives `sessions_over_depth`, `model_overthinking`, `overpowered_subagents`, `old_model_usage`, `overuse_of_fast_mode`, and `cache_churn` from typed session evidence. It keeps findings from each source distinct if they share a check ID. The CLI does not implement those evidence-detector rules.

The CLI also returns the `session_price_outlier` source. It reprices cached `session_analysis.pricing_breakdown_json` with the adjacent `model-pricing.json` snapshot; it does not read a stored high-cost flag. The cohort contains the target and up to 15 neighboring sessions within 72 hours of the target's last-activity timestamp. The CLI orders neighbors by time distance, then activity recency. It selects at most 16 sessions before checking whether their prices are usable, so it does not reach farther back to replace an unpriced neighbor. At least 8 sessions must have fully priced breakdowns. The target is flagged only when its estimated cost is strictly greater than `max($2, 3 × median(cohort costs))`. The defaults match the desktop's numerical threshold, but the bounded target-centered cohort can differ from the desktop's visible activity-list cohort.

This derived result is an estimate, not a bill or a currentness claim. The CLI reads the last cached pricing snapshot and does not refresh it. If the target, pricing snapshot, or minimum cohort is unavailable, the CLI does not report a cost-outlier finding or present the session as clean.

By default, the CLI looks for `antiburn.sqlite3` under `ai.antiburn.desktop` in the platform data directory. It assumes release-build values and does not fall back to debug paths. The database is opened with SQLite read-only flags and `query_only` enabled. `antiburn-local` does not expose a reader for Antiburn's product database; its read-only source-database readers are for agent session formats. The desktop's product `Store` lives in the Tauri app, so the standalone CLI keeps a small read-only query layer for the product tables. The session ID comes only from the command argument; the CLI does not read `PI_SESSION_ID` or another harness variable. It fails closed if the ID matches multiple agent/environment identities.

The optional JSON config lives at `dirs::config_dir()/antiburn/cli.json`. Pass `--config PATH` to use a different file. Omitted values use the release defaults. For example, this config points to a debug database:

```json
{
  "data_root": "/Users/example/Library/Application Support",
  "app_identifier": "ai.antiburn.desktop.debug",
  "database_filename": "antiburn-debug.sqlite3",
  "busy_timeout_ms": 5000,
  "state_file": "/Users/example/.config/antiburn/cli-state.json",
  "pricing_snapshot_path": "/Users/example/Library/Application Support/ai.antiburn.desktop/model-pricing.json",
  "price_outlier": {
    "max_sessions": 16,
    "max_age_hours": 72,
    "minimum_priced_sessions": 8,
    "floor_usd": 2.0,
    "median_multiple": 3.0
  }
}
```

`data_root` is the directory before the app identifier. The identifier and database filename must each be one path component. If `pricing_snapshot_path` is absent, the CLI looks for `model-pricing.json` beside the selected database. The `price_outlier` values override the cohort and threshold defaults. The CLI creates its state file when it reserves delivery after a successful lookup. It does not create config, data-root, database, or pricing files. Unknown config fields are errors. On macOS, the default platform data directory is `~/Library/Application Support`. On Linux, it is `$XDG_DATA_HOME` or `~/.local/share`. On Windows, it is the roaming application-data directory.

The CLI initializes a JSON Lines tracing subscriber during startup. It appends structured events to `target/debug/calls.log` beside the executable. A successful lookup records the explicit session ID, evidence status, and number of new findings returned. It does not log evidence JSON, check results, prompts, or transcripts. If file tracing is unavailable, the CLI warns on stderr and continues. Do not commit this generated log.

Run the debug binary by its full path from this crate:

```sh
cargo build
./target/debug/antiburn-cli capabilities --json
```

Checks write their own local diagnostic log. Capabilities does not initialize file
logging or load config. Source builds do not install a shell command automatically.

## Agent session identity: harness findings

Research snapshot: **2026-10-16**. These entries describe current official documentation or source checked for Antiburn's supported inventory. Environment markers are not necessarily session IDs. “Not documented” means no session-ID variable appeared in the sources checked; it does not prove that no implementation exposes one.

| Harness | Environment available to commands | Session-ID guidance |
| --- | --- | --- |
| Pi | `PI_SESSION_ID` and `PI_SESSION_FILE` are added to built-in Bash and PowerShell tool commands. `AI_AGENT=pi` and `PI_CODING_AGENT=true` identify Pi-launched processes, not a session. | Pi exposes `PI_SESSION_ID` for shell-tool pass-through tests, but the Antiburn skill passes the active session ID as the CLI argument. These variables are not injected into user-entered `!` or `!!` commands. [Environment variables](https://pi.dev/docs/latest/environment-variables) |
| Claude Code | The documentation describes `CLAUDE_CODE_SESSION_ID` for Bash/PowerShell commands, hook commands, and stdio MCP servers. Do not assume it reaches every arbitrary child process. | Hook JSON explicitly includes `session_id`, and `/clear` updates the session. [Environment variables](https://docs.anthropic.com/en/docs/claude-code/env-vars), [hooks](https://code.claude.com/docs/en/hooks) |
| Codex CLI | Current Codex source injects `CODEX_SESSION_ID` for shell commands and adds `CODEX_THREAD_ID` when it has a thread ID. | Treat these as shell-tool variables; command hooks also provide `session_id` in their stdin JSON. These names are established by source, not the public shell-environment list. [Session injection](https://github.com/openai/codex/blob/main/codex-rs/core/src/exec_env.rs), [shell environment](https://github.com/openai/codex/blob/main/codex-rs/protocol/src/shell_environment.rs), [hooks](https://developers.openai.com/codex/hooks) |
| Cursor | `CURSOR_AGENT` identifies an Agent terminal. It is a marker, not the conversation ID. | No session-ID shell variable is documented in the sources checked. Command hooks receive `conversation_id` and `generation_id`. [Terminal](https://docs.cursor.com/en/agent/terminal), [hooks](https://cursor.com/docs/hooks) |
| GitHub Copilot CLI | The CLI documents configuration variables such as `COPILOT_HOME` and `COPILOT_MODEL`; no session-ID command environment variable is listed. | Use `/session` interactively or read the field for the configured hook schema: Copilot CLI hooks use `sessionId`, while VS Code-compatible hooks use `session_id`. [CLI environment](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-programmatic-reference), [hooks](https://docs.github.com/en/copilot/reference/hooks-reference) |
| Cline | Variables such as `CLINE_HOOKS_DIR` configure the CLI; no session-ID command environment variable is documented. | Plugins can receive optional `ctx.session?.sessionId`. File hooks use their own JSON payload and expose `taskId`, which must not be assumed to be Antiburn's database identity. [CLI reference](https://github.com/cline/cline/blob/main/docs/cli/cli-reference.mdx), [plugin context](https://github.com/cline/cline/blob/main/sdk/.cline/skills/plugin.md) |
| OpenCode | A plugin's `shell.env` hook can add custom variables. The documented hook input includes `cwd`, not a session ID. | A plugin can read a session event such as `session.idle` (`event.properties.sessionID`) and pass the ID explicitly to a child command. The event is in-process, not CLI-hook stdin. [Plugins](https://dev.opencode.ai/docs/plugins/) |
| Kiro CLI | Kiro documents `KIRO_HOME`, but no session-ID environment variable. | `/session-id` shows the active ID. Command hooks receive JSON on stdin; `session_id` is documented on `SessionStart`/`agentSpawn`, but not in the documented Agent Stop payload. [Settings](https://kiro.dev/docs/cli/reference/settings/), [hook types](https://kiro.dev/docs/hooks/types/) |
| Amp | No general thread-ID environment variable is documented. `AMP_THREAD_ID` is specific to Orb portals, not a general shell contract. | An Amp plugin's `agent.end` event exposes `event.thread.id`; pass that value to a child command. [Plugin API](https://ampcode.com/docs/plugin-api), [portals](https://ampcode.com/docs/orbs/portals) |
| Google Antigravity | No conversation-ID environment variable is documented for hook commands. | Hook stdin includes `conversationId`. The status-line API uses `conversation_id` and documents `session_id` as a backward-compatible alias. [Hooks](https://antigravity.google/docs/hooks), [status line](https://antigravity.google/docs/cli/statusline/) |
| Windsurf Cascade | No session-ID environment variable is documented for hooks. | Hook stdin includes `trajectory_id` for the conversation and `execution_id` for the current turn. [Cascade hooks](https://docs.windsurf.com/windsurf/cascade/hooks) |

The reusable Antiburn skill should stay simple: call `antiburn-cli checks <session_id>` with the active session ID supplied by the harness. The positional argument is the CLI's source of truth; do not read a harness-specific session environment variable. Do not make the skill guess an agent name or Antiburn environment key. The CLI resolves agent and environment from Antiburn's local store and fails closed if the ID is missing or ambiguous. The CLI returns only new or changed findings; an empty `checks` array means the skill has nothing to report.

## Hook input is not one shared contract

Command hooks often receive JSON on stdin, but field names and output rules differ. A CLI that runs under a hook needs a small harness adapter: read the documented input, select the correct ID, call `antiburn-cli checks <id>`, then translate the result into that harness's hook response. Do not return Antiburn's JSON unchanged unless that hook explicitly accepts it as context. Hook stdout may be interpreted as a control response, not displayed text.

| Harness | End-of-turn hook input | What to extract |
| --- | --- | --- |
| Claude Code `Stop` | `{"hook_event_name":"Stop","session_id":"…","transcript_path":"…","cwd":"…","stop_hook_active":false}` | `session_id` [Stop schema](https://code.claude.com/docs/en/hooks#stop) |
| Codex `Stop` | `{"hook_event_name":"Stop","session_id":"…","turn_id":"…","cwd":"…","stop_hook_active":false,"last_assistant_message":"…"}` | `session_id`; `turn_id` is turn-scoped. [Stop schema](https://developers.openai.com/codex/hooks) |
| Codex legacy `notify` | The `agent-turn-complete` JSON is appended as the command's final argv item, not written to stdin: `{"type":"agent-turn-complete","thread-id":"…","turn-id":"…","cwd":"…","input-messages":[],"last-assistant-message":"…"}`. | `thread-id`. Do not parse stdin for this legacy notification. [Source](https://github.com/openai/codex/blob/main/codex-rs/hooks/src/legacy_notify.rs) |
| Cursor `stop` | `{"hook_event_name":"stop","conversation_id":"…","generation_id":"…","status":"completed","loop_count":0}` | `conversation_id`; `generation_id` changes per generation. [Stop schema](https://cursor.com/docs/hooks) |
| Copilot CLI `agentStop` | `{"sessionId":"…","timestamp":0,"cwd":"…","transcriptPath":"…","stopReason":"end_turn","stop_hook_active":false}` | The Copilot CLI hook schema uses camelCase `sessionId`; the VS Code-compatible hook schema uses snake_case `session_id`. [Hook input](https://docs.github.com/en/copilot/reference/hooks-reference) |
| Cline CLI `TaskComplete` | `{"hookName":"TaskComplete","taskId":"…","workspaceRoots":["…"],"taskComplete":{"taskMetadata":{"taskId":"…","ulid":"…"}}}` | `taskId` is Cline task context, not a proven Antiburn session identity. [Hook input](https://github.com/cline/cline/blob/main/.clinerules/hooks/README.md) |
| Kiro `Agent Stop` | Command actions receive JSON on stdin. The documented `agentSpawn` example is `{"hook_event_name":"agentSpawn","cwd":"…","session_id":"…"}`; the Agent Stop page does not define an equivalent session field. | Do not assume Agent Stop includes `session_id` until a runtime test confirms it. [Hook types](https://kiro.dev/docs/hooks/types/), [actions](https://kiro.dev/docs/hooks/actions/) |
| Windsurf `post_cascade_response` | `{"agent_action_name":"post_cascade_response","trajectory_id":"…","execution_id":"…","timestamp":"…","tool_info":{"response":"…"}}` | `trajectory_id` identifies the conversation. The response can contain private conversation and tool content. [Hook schema](https://docs.windsurf.com/windsurf/cascade/hooks) |
| Antigravity `Stop` | `{"executionNum":1,"terminationReason":"model_stop","error":"","fullyIdle":true,"conversationId":"…","workspacePaths":["…"],"transcriptPath":"…","artifactDirectoryPath":"…","modelName":"…"}` | `conversationId`; the hook can also run on errors or while work is not fully idle. [Hook schema](https://antigravity.google/docs/hooks) |
| OpenCode plugin | In-process session event: `{"type":"session.idle","properties":{"sessionID":"…"}}`. | `event.properties.sessionID`; no generic command-hook stdin payload. [Plugin events](https://dev.opencode.ai/docs/plugins/) |
| Amp plugin `agent.end` | In-process event contains `event.thread.id` and `event.status`. | Pass `event.thread.id` to the CLI; no external JSON stdin payload. [Event API](https://ampcode.com/docs/plugin-api) |
| Pi extension `agent_end` | In-process extension context exposes `ctx.sessionManager.getSessionId()`. | Pass the ID explicitly to the CLI; no separate shell-hook JSON payload. [Extensions](https://pi.dev/docs/latest/extensions) |

Hook schemas change. Verify the installed harness version and use a session ID from the active session, not a transcript path, turn ID, or process marker. The JSONL transcript and hook payload can contain private prompts, assistant text, file paths, and tool data; this CLI does not read or log them.

## Aislop

`apps/cli/.aislop/config.yml` was generated from the installed `aislop 0.18.1` `aislop init --strict` defaults. It enables all engines, type checking, security audit, and a CI score floor of 85. The settings are scoped to this app: Aislop selects the nearest `.aislop` directory when the target is `apps/cli`.

Run the strict app check from the repository root:

```sh
pnpm exec aislop ci apps/cli
```

The generated configuration enables Aislop telemetry, matching `init --strict`. The root `.aislop` config remains unchanged.

## Finding state and output

The state file defaults to `dirs::config_dir()/antiburn/cli-state.json`. Set `state_file` in `cli.json` to override it. The state path is independent of `--config PATH` unless the config sets `state_file`. The CLI keys each session by `(environment_key, agent, session_id)` and each check by `(source, check_id)`, so matching IDs from different stores or sources do not collide. On each successful delivery, it records the current Unix time as `last_checked_at_epoch`. Evidence badges use their outcome; price outliers use their categorical status, so changing cost, threshold, or cohort size does not repeat an existing outlier. Persisted checks fingerprint their findings, excluding result-level revisions, coverage, usage counters, and finding publication/model metadata. Changed finding content still triggers output. Confirmed clean results permit a later recurrence; missing, running, failed, and not-assessed checks preserve deduplication across unknown availability.

The state file stores check statuses, SHA-256 fingerprints, the last triggered status/fingerprint/time, and the last-check time. It does not store raw evidence or full check results. State writes use a nonblocking lock and atomic replacement through `std::fs::rename` on Unix and Windows. Replacement errors retain the existing file. A busy lock fails promptly with a retry error. A delivery reservation lasts 60 seconds and contains only the proposed fingerprint state. Another call for the same session fails with a retry error while the reservation is active; other sessions can proceed. Finding suppression advances only after JSON output writes and flushes successfully. Confirmed clean transitions persist when delivery is reserved, even if output later fails or the process exits. Output failure releases the reservation; a killed process leaves an expiring reservation. Expired receipts cannot acknowledge newer deliveries. A crash after flush but before acknowledgement, a failed acknowledgement, or a delivery exceeding the lease can cause duplicate output on retry. A successful flush proves operating-system output acceptance, not downstream consumption. No exactly-once guarantee is possible without a consumer acknowledgement. Existing schema-1 files remain readable, but changed fingerprint semantics can report an existing finding once after upgrading. Older binaries reject files containing delivery reservations. A database or state error fails the command. Stdout omits raw session evidence. The CLI still does not evaluate the desktop's full result-currentness rules. The canonical harness-neutral skill is `skills/antiburn/SKILL.md`; the release-package helper copies it to the archive root as `SKILL.md`. Installation and support in other harnesses remain separate work.

## Predefined check delivery (development)

The CLI reads saved predefined YAML check outcomes in addition to existing
assessment and badge sources. Their source label is `predefined_yaml`. The
ordinary command still returns only new or changed problematic results and
keeps its existing JSON envelope and state file. It does not return clean
results or raw evidence.

Native plugins use a separate versioned protocol:

```sh
antiburn-cli checks example-session --delivery-v1 --consumer claude-code:context
antiburn-cli checks example-session --delivery-v1 --consumer claude-code:context --ack RECEIPT
```

The first call returns `protocol_version: 1`, the database-resolved `identity`,
`session_scope`, a `state`, a `snapshot`, `updates`, and an optional `receipt`.
The snapshot includes safe check metadata, authored status messages, assessment
times, and checking activity. Updates contain `finding`, `cleared`, or
`invalidated`; unchanged failures and routine passing assessments do not create
context updates. A finding requires an authored message. Invalidated results
are withdrawn without claiming repair.

A consumer acknowledges its receipt only after handing the update to its native
host. Until acknowledgement, retries can replay the batch. A crash between
handoff and acknowledgement can also replay it. An obsolete receipt cannot
replace newer pending state. UI and context use distinct consumer names;
ordinary CLI reads do not consume either stream. Consumers ending in `:status`
read diagnostic snapshots without staging or acknowledging delivery updates. The separate, locked state
file is the configured state filename plus `.delivery-v1`; it stores only
fingerprints and receipts. A bounded lock wait fails rather than blocking a
host callback indefinitely.

The read-only shared projection applies activation, definition revision,
generation, scope, source readiness, and evidence validity gates. Its runtime
metadata is written by Antiburn, never by the CLI. Older databases continue to
support ordinary CLI checks; the delivery protocol requires the new projection
schema. Before the app saves its first discovery snapshot, delivery reports
`waiting`; an initialized empty snapshot reports `unconfigured`.
The CLI does not discover YAML files, evaluate providers, start the
app, or expose an HTTP/IPC service.

`freshness_evaluated` remains false. Saved eligible results do not prove the
app is running, that definitions on disk are unchanged, or that an assessment
covers the latest activity. Native context must identify this limitation.
Missing databases, incompatible projection schemas, unsupported session scopes,
disabled checks, unconfigured checks, and waiting states are distinct from a
successful empty assessment. Ambiguous or unknown IDs fail closed.
