# Burn Check Source Coverage

Audit date: 2026-09-15.

This document covers local passive evidence only. Coverage must not use hooks,
new extensions, runtime subscriptions, or agent calls to fill evidence gaps. Existing
persisted output from the reviewed Pi example extension is a passive input. The
current OpenCode WSL discovery conflict is recorded in `session-coverage.md`.

See [`session-coverage.md`](session-coverage.md) for discovery, framing, parsing,
companion-source, and provider-route coverage for the same source formats.

## Status Rules

| Status      | Meaning                                                                                                                                           |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Assessable  | The current reader can produce the evidence needed for a finding and a clean result. A damaged or incomplete session can still be partial.        |
| Partial     | The source has useful passive evidence, but the current reader or the source cannot prove all required facts. Do not report a clean result.       |
| Unsupported | The reviewed passive sources do not prove a required fact or its policy semantics. This is source-scoped, not a claim about future formats.       |
| Unknown     | The source or its relevant field semantics are not characterized. Do not infer support from a path, field name, mode name, or generic JSON shape. |

`Partial` means the implemented reader can produce a bounded positive finding,
but cannot prove every fact required for a clean result. An unimplemented or
uncharacterized evidence path is `Unknown` or `Unsupported`, not `Partial`.
`Assessable` describes the accepted source contract, not every session or
historical release. Complete session facts, eligible activity, and reviewed
model/provider policy remain necessary for clean.

## Checks

| Code | Check                 |
| ---- | --------------------- |
| D    | Session overdepth     |
| T    | Model overthinking    |
| S    | Overpowered subagents |
| M    | Unused MCP servers    |
| B    | Unused built-in tools |
| K    | Unused skills         |
| O    | Old model usage       |
| F    | Fast mode overuse     |
| C    | Cache churn           |

## Source Inventory

The tables list all 30 `SourceFormat` keys. Known source shape and release
version are separate facts. A version range is not always available; an accepted
schema, header, or pinned producer commit with synthetic fixtures can establish
a bounded contract. No row promises parity across all historical versions.

| `SourceFormat`                 | Passive source format                                               | Version statement                                                                                                                                                 | Current reader                        |
| ------------------------------ | ------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| `ClaudeJsonl`                  | Claude Code session JSONL and child sidecars                        | Private 2.1.220-2.1.246 observation contract; main JSONL and sidecar shapes are pinned separately                                                                 | Dedicated                             |
| `CodexRolloutJsonl`            | Codex rollout JSONL, with discovered child rollouts                 | Recorder commit `e7637306bc9246a3e42e407cb94f96b7ed345e3e`; synthetic fixtures                                                                                    | Dedicated                             |
| `OpenCodeJsonl`                | OpenCode legacy exported session data                               | Accepted export wrappers and native message/part shapes; pinned research below                                                                                    | Dedicated                             |
| `OpenCodeSqliteV2`             | OpenCode SQLite `session`, `message`, `part` tables                 | Fixture-backed required-column contract in a read-only transaction snapshot; not CoreV2 `session_message`                                                         | Dedicated                             |
| `PiV3Jsonl`                    | Pi session JSONL                                                    | Leading header version 3 and pinned core/example-extension shapes; headerless and unsupported-version sources are rejected                                        | Dedicated                             |
| `CursorJsonl`                  | Cursor compatibility JSONL without a surface marker                 | Unversioned and uncharacterized                                                                                                                                   | Dedicated shared Cursor reader        |
| `CursorCliAgentJsonl`          | Cursor agent transcript JSONL                                       | Separate partial export contract; no model fallback                                                                                                               | Dedicated shared Cursor reader        |
| `CursorCliStoreDb`             | Legacy Cursor CLI `chats/**/store.db` data                          | Private `blobs`/`meta` subset pinned by public reverse engineering; partial                                                                                       | Dedicated shared Cursor reader        |
| `CursorChatStoreDb`            | Cursor chat `~/.cursor/chats/<workspace>/<session>/store.db` data   | Chat path and private `blobs`/`meta` subset pinned by public reverse engineering; partial                                                                         | Dedicated shared Cursor reader        |
| `CursorIdeComposer`            | Cursor IDE composer data from `state.vscdb`                         | Private and unversioned; current synthesis is partial                                                                                                             | Dedicated shared Cursor reader        |
| `CursorLegacyChatJson`         | Cursor IDE `chatSessions/*.json`                                    | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed profile         |
| `AntigravityJson`              | Internal Antigravity compatibility profile                          | Not emitted by current source classification                                                                                                                      | Dedicated shared profile              |
| `AntigravityBrainJsonl`        | Antigravity brain transcript JSONL                                  | Unversioned; current shape is partially characterized                                                                                                             | Dedicated                             |
| `AntigravityCascadeJson`       | Antigravity API cascade or mirror JSON                              | Unversioned; current shape is partially characterized                                                                                                             | Dedicated                             |
| `AntigravityWorkspaceChatJson` | Antigravity workspace `chatSessions/*.json`                         | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed profile         |
| `AntigravitySqlite`            | Native `conversations/<uuid>.db` plus an optional brain transcript  | agy 1.0.16 reverse-engineered subset; requires `user_version = 1` and reviewed `gen_metadata(idx,data)` or `steps(idx,metadata)` columns; not full schema support | Dedicated                             |
| `CopilotCliJsonl`              | `session-state/<uuid>/events.jsonl`                                 | Public Copilot SDK v1 envelope; fixture-backed `session.start` and persisted `session.shutdown` contract                                                          | Dedicated v1 CLI reader               |
| `CopilotIdeChatJson`           | VS Code-family `chatSessions/*.json`                                | Unversioned; IDE and CLI contracts are separate                                                                                                                   | Dedicated fail-closed                 |
| `ClineSessionJson`             | Cline metadata and message companion                                | Cline 2.0+ naming is known; message schemas are not pinned                                                                                                        | Dedicated fail-closed                 |
| `ClineMessagesContractV1`      | Cline terminal `sessions.db`, root manifest, and messages artifacts | Cline messages-contract v1; fixture-backed required `sessions` columns and canonical root/child artifacts                                                         | Dedicated v1 bundle reader            |
| `KiroSessionJson`              | Kiro workspace-session JSON                                         | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed                 |
| `KiroChat`                     | Kiro `.chat` fallback                                               | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed                 |
| `KiroCliV2Bundle`              | Kiro CLI V2 `.json` metadata and matching `.jsonl` journal          | Fixture-backed observed V1 metadata and V1 envelope contract; UUID siblings only                                                                                  | Dedicated V2 bundle reader            |
| `KiroCliV3Bundle`              | Kiro CLI V3 `session.json` and `messages.jsonl` directory           | Separate path is known, but no pinned `session.json` producer shape                                                                                               | Dedicated fail-closed                 |
| `KiroChatSaveExport`           | Manual Kiro CLI `/chat save` JSON                                   | Public command is known; export schema is not published                                                                                                           | Not scanned; unsupported              |
| `AmpThreadJson`                | Amp `threads/*.json` whole-thread record                            | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed                 |
| `AmpFileChanges`               | Amp `file-changes/**/*.{json,jsonl}`                                | File-change fallback, not a thread                                                                                                                                | Dedicated fail-closed                 |
| `WindsurfWorkspaceJson`        | Windsurf workspace chat JSON                                        | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed                 |
| `WindsurfMirrorJson`           | Configured Windsurf mirror JSON                                     | Unversioned and uncharacterized                                                                                                                                   | Dedicated fail-closed                 |
| `WindsurfCascadeProtobuf`      | Windsurf Cascade `.pb` data                                         | Private and uncharacterized                                                                                                                                       | Dedicated fail-closed when discovered |
| `Uncharacterized`              | Unknown-agent generic fallback                                      | No source contract                                                                                                                                                | Generic fail-closed                   |

## Coverage Matrix

This manual matrix records implemented eligibility and audited source limits,
not just binary capability flags. The inventory test checks keys and cell
vocabulary; behavior tests separately check finding and clean gates.

| `SourceFormat`                 | D           | T           | S           | M           | B           | K           | O           | F           | C           |
| ------------------------------ | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- |
| `ClaudeJsonl`                  | Assessable  | Assessable  | Assessable  | Partial     | Partial     | Partial     | Assessable  | Assessable  | Assessable  |
| `CodexRolloutJsonl`            | Assessable  | Assessable  | Assessable  | Partial     | Partial     | Partial     | Assessable  | Assessable  | Assessable  |
| `OpenCodeJsonl`                | Assessable  | Unsupported | Assessable  | Unsupported | Unsupported | Partial     | Assessable  | Unsupported | Assessable  |
| `OpenCodeSqliteV2`             | Assessable  | Unsupported | Assessable  | Unsupported | Unsupported | Partial     | Assessable  | Unsupported | Assessable  |
| `PiV3Jsonl`                    | Assessable  | Assessable  | Partial     | Unsupported | Unsupported | Unsupported | Assessable  | Unsupported | Assessable  |
| `CursorJsonl`                  | Unsupported | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Partial     | Unknown     | Unknown     |
| `CursorCliAgentJsonl`          | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorCliStoreDb`             | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorChatStoreDb`            | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorIdeComposer`            | Unsupported | Unknown     | Unsupported | Unsupported | Unsupported | Unknown     | Partial     | Unknown     | Unsupported |
| `CursorLegacyChatJson`         | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `AntigravityJson`              | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AntigravityBrainJsonl`        | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AntigravityCascadeJson`       | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `AntigravityWorkspaceChatJson` | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `AntigravitySqlite`            | Partial     | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `CopilotCliJsonl`              | Unsupported | Unsupported | Assessable  | Unsupported | Unsupported | Unsupported | Assessable  | Unsupported | Unsupported |
| `CopilotIdeChatJson`           | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Partial     | Unknown     | Unknown     |
| `ClineSessionJson`             | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `ClineMessagesContractV1`      | Unsupported | Unsupported | Partial     | Unsupported | Unsupported | Unsupported | Partial     | Unsupported | Unsupported |
| `KiroSessionJson`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroChat`                     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroCliV2Bundle`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroCliV3Bundle`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `KiroChatSaveExport`           | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AmpThreadJson`                | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `AmpFileChanges`               | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `WindsurfWorkspaceJson`        | Unknown     | Unknown     | Unknown     | Partial     | Partial     | Unknown     | Partial     | Unknown     | Unknown     |
| `WindsurfMirrorJson`           | Unknown     | Unknown     | Unknown     | Partial     | Partial     | Unknown     | Partial     | Unknown     | Unknown     |
| `WindsurfCascadeProtobuf`      | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |
| `Uncharacterized`              | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     | Unknown     |

## Evidence Boundaries

Burn checks use only sessions admitted by the repository scan gate. A session
needs a resolvable Git repository CWD. Disabled roots and their linked
worktrees are excluded before evidence processing; missing or unresolved CWDs
are unavailable, never clean.

No current reader proves a full historical resource inventory. All M/B/K checks
deny session-wide `Clean`, even when a nested observed-resource map is complete.
A scoped finding requires complete coverage of that observed subset, calls, and
eligible activity. An unrelated partial resource group does not block it.

Report-time token estimates (`insights/report.rs::token_cost` and
`TokenBurnTurnEvidence`), old-model remediation savings, and provider-limit
attribution all read `turn.cache_write_1h_tokens` and price that subset at
two times the input rate. These readers are report-time views of turn rows,
not part of `SessionEvidence`, so the evidence schema revision does not
change when this pricing split changes.

Thread attribution retains at most 16,384 distinct UUIDs, each at most 256 bytes.
An overflow or oversized UUID records `CapExceeded`, makes attribution
incomplete, and blocks every clean result that needs complete affected evidence.
An oversized resume identity set is rejected instead of being trusted.

Codex pairs `token_usage_record` and `event_msg`/`token_count` records once.
Matching per-response and nonempty cumulative usage identifies exact copies
without a time limit. Different cumulative producer bases require matching
per-response usage within five seconds. Available identities distinguish
same-format requests. The reader retains bounded state across resume boundaries.
Unmatched valid usage remains evidence; malformed usage remains partial.

Cache accounting compares usage-bearing requests across ordinary Codex assistant
messages. Broken links, unknown requests, route changes, and compaction boundaries
still break pairs. The paid denominator includes every eligible request,
including each segment's initial payment. Partial cache or repeated-context
evidence permits neither a ratio finding nor a clean result.

Maintainer confirmation (2026-09-12): repair delayed exact-copy deduplication,
request pairing, and full-denominator accounting. Reviewed passive alternatives
include increasing the time limit and adjusting thresholds; neither repairs
all three accounting errors. Per-session thresholds remain unchanged.

Maintainer confirmation (2026-09-10): raise the identity cap to 16,384 as an
interim measure for longer sessions. Reviewed passive alternatives include
indexed local relationship queries and bounded batch processing. Those
alternatives remain outside this change; sessions above the cap still lose
clean-result eligibility. Analyzer revision 22 reprocesses prior evidence.

Skills mean full documents injected into model context. Listings, installed
skills, and names in tool calls do not prove unused document overhead. Resource
identity is retained without copying private document bodies into evidence.

| Source                      | Checks              | Implemented contract and remaining limit                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| --------------------------- | ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude and Codex            | D, O                | Direct request depth and timed model use reach checks independently of token-accounting policy. Clean needs all required session facts and reviewed model state. Unknown models are not automatically current.                                                                                                                                                                                                                                                                                                   |
| Claude and Codex            | T, F                | Request-level model, effort, speed, and route observations are evaluated together. Codex preserves explicit provider/control changes and inherited fork state; child controls retain delegated scope. Missing eligible signals or unreviewed routes deny clean.                                                                                                                                                                                                                                                  |
| Claude                      | S                   | Exact `Task`/`Agent` call IDs join unique sidecar `toolUseId` claims to actual child models and the model on the parent call. Missing, duplicate, malformed, mismatched, or nested-parent claims remain partial. Requested model aliases and directory ancestry are not proof.                                                                                                                                                                                                                                   |
| Codex                       | S                   | Owned `spawn_agent` records and discovered child rollouts provide delegation and actual models. Incomplete child evidence cannot prove clean.                                                                                                                                                                                                                                                                                                                                                                    |
| Claude                      | M, K                | Observed MCP injection plus exact server calls and full skill-document injection plus invocation identity support scoped findings. Skill listings alone do not. The observed subset is not a historical inventory.                                                                                                                                                                                                                                                                                               |
| Codex                       | M                   | Accepted completed client `tool_search_output` namespace records expose exact MCP server identities. Complete observed exposure and calls support scoped findings without a full-inventory capability. Ambiguous or incomplete search results do not establish injection.                                                                                                                                                                                                                                        |
| Claude and Codex            | B                   | The existing harness-version/model catalog path supports scoped definition findings with complete calls. Deferred, situational, and zero-cost definitions are excluded. Catalog resolution does not prove every historical enabled tool or exposure change; it cannot justify a whole-inventory claim.                                                                                                                                                                                                           |
| Codex                       | K                   | Selected full skill documents reach observed injection/invocation evidence. Listings remain availability only. Selected documents do not establish unused listing overhead or full inventory coverage.                                                                                                                                                                                                                                                                                                           |
| OpenCode                    | S                   | Native `task` metadata identifies the child session and model; ancestry and the child's assistant model must agree. The parent model comes from the task request. A bare `subtask`, fork, or `parent_id` relation is insufficient.                                                                                                                                                                                                                                                                               |
| OpenCode                    | K                   | Complete native selected-skill results preserve full identity as injected and invoked. Truncated, compacted, empty, or invalid result wrappers do not prove full injection. This is observed selected-skill support, not an unused-listing finding or complete inventory.                                                                                                                                                                                                                                        |
| OpenCode                    | T, M, B, F          | Confirmed unsupported for the reviewed sources: no historical effort map, model-facing resource inventories, or effective speed tier. The reader does not retain a variant as effort. Variant labels, current configuration, and tool registries cannot substitute.                                                                                                                                                                                                                                              |
| Pi                          | T                   | `EffortSemantics::AgentSelectedPolicy` evaluates the saved agent-selected thinking level, not translated provider effort. Reviewed native routes and branch/fork policy state are retained. Missing levels/routes and unknown models fail closed; provider overrides are not guessed.                                                                                                                                                                                                                            |
| Pi                          | S                   | Existing output from the official subagent example extension supplies nested `toolResult` messages, exact native call/worker identity, and actual models. This is finding-only. Arbitrary extensions, fork ancestry, requested aliases, and a nonpremium observed worker cannot establish clean.                                                                                                                                                                                                                 |
| Pi                          | M, B, K, F          | Confirmed unsupported for the reviewed sources. Tool calls and bounded skill invocation identity do not establish historical resource exposure or speed. No alternative local proof was identified.                                                                                                                                                                                                                                                                                                              |
Quota pressure and provider incidents sit outside the nine-code check
contract (FR-15): neither has a row in the Checks table above, and each
reports only when transcripts carry its own evidence. `CodexRolloutJsonl`
and `ClaudeJsonl` both supply quota and provider incidents.

`CodexRolloutJsonl`: an `event_msg`/`task_complete` record with a non-null
`error` object maps to one of the two groups, against the pinned
`openai/codex` protocol commit
[`e7637306bc9246a3e42e407cb94f96b7ed345e3e`][codex-source] and a synthetic
fixture (`task_complete_errors.jsonl`):

- `quota_incidents`, a `QuotaIncident`, from `rate_limit_exceeded`
  (`RateLimit`) and `usage_limit_exceeded` (`UsageLimit`) — both name a
  user-allocation limit the reader's own usage caused.
- `provider_incidents`, a `ProviderIncident`, from `server_overloaded`
  (`Capacity`), `internal_server_error` (`ServerError`), and the four
  transport struct-variant codes `http_connection_failed`,
  `response_stream_connection_failed`, `response_stream_disconnected`, and
  `response_too_many_failed_attempts` by their `http_status_code`: `5xx`
  maps to `ServerError`, an absent or `null` status maps to `Connection`,
  and any other status is ignored because the retry wrapper hides which
  layer produced it.

Every other Codex code (`context_window_exceeded`, `session_budget_exceeded`,
`cyber_policy`, `misalignment_policy_violation`, `unauthorized`,
`bad_request`, `sandbox_error`, `active_turn_not_steerable`,
`thread_rollback_failed`, `other`) is ignored.

`ClaudeJsonl`: a `type: "assistant"` record with `isApiErrorMessage: true`
maps to one of the two groups from its `apiErrorStatus` (an HTTP status,
present only for a response the provider returned) and `error` (Claude
Code's own coarser classification), reviewed against harness version
`2.1.270` and a synthetic fixture (`api_error_records.jsonl`):

- `quota_incidents`, a `QuotaIncident` (`RateLimit`, `HardHit`), from status
  `429` or, when no status is present, `error: "rate_limit"`.
- `provider_incidents`, a `ProviderIncident`, from status `529` (`Capacity`),
  another `5xx` status (`ServerError`), or, when no status is present,
  `error: "server_error"` (`ServerError`).

Every other status or `error` value is ignored, including `error: "unknown"`
with no status (Claude's connection-refused case) and every 4xx other than
429. `ProviderIncidentKind::Connection` is Codex-only: no Claude field
reviewed so far identifies a connection failure without reading message
text.

Clean or absence is never claimed from either group for any source: each
section is not assessed without at least one observed incident of its own
kind, per FR-15's one condition.

Maintainer confirmation (2026-09-14): extend provider incidents with
`ServerError` and `Connection`, map Codex's remaining transport/server
`codex_error_info` codes, and add Claude `isApiErrorMessage` records as a
new quota/provider incident source. Reviewed passive alternatives:

- Classifying Claude errors from `content[].text` — rejected: free text,
  unpinned, and the project never reads or stores error message text.
- Mapping Claude `error: "unknown"` to `Connection` — rejected: the label
  covers more than connection failures.
- Mapping non-5xx `http_status_code` values inside Codex transport
  variants — rejected: the retry wrapper hides which layer produced the
  status.
- Mapping `context_window_exceeded` / `session_budget_exceeded` — rejected:
  these name the user's own context or budget, a different failure class.
- Splitting Claude 429s into `UsageLimit` vs `RateLimit` from
  `quotaLimits` — deferred: no synthetic fixture has been characterised for
  that field yet.

| Claude, Codex, OpenCode, Pi | C                   | Durable request provider/API fields and the compatible-request query select reviewed cache-write or uncached-input accounting. Main-thread identity, order, token classes, model, route, and compaction boundaries constrain pairs. Codex pairs `token_usage_record` with equivalent `token_count` usage by per-response fields; matching cumulative fields permit delayed exact copies. Unknown or incompatible segments prevent both ratio findings and clean results. Google cache policy remains unreviewed. |
| OpenCode                    | C                   | Both accepted export and SQLite shapes use validated ordered history. `parentID` identifies the user being answered, not the predecessor. Missing wrappers/timestamps, duplicate or out-of-order messages, and unresolved forks prevent complete history. CoreV2 `session_message` is not the existing SQLite table contract.                                                                                                                                                                                    |
| Cursor                      | D, O                | O retains direct timed-model findings; the source gate denies clean on every surface. D remains unavailable because the current reader does not emit request-usage evidence. Synthetic source-gate tests do not establish native parsing support.                                                                                                                                                                                                                                                                |
| Cursor                      | T, S, M, B, K, F, C | Broader surface characterization is deferred. Current settings, relations, inventories, and cache evidence remain partial, unknown, or unsupported as listed; no new parity claim is made.                                                                                                                                                                                                                                                                                                                       |
| Antigravity                 | D, O                | Brain/cascade steps and native SQLite preserve direct usage/model findings where present. Missing model/time is not filled from an earlier step or an invented database timestamp. Private identity, enum, and completeness gaps deny clean.                                                                                                                                                                                                                                                                     |
| Antigravity                 | T, S, M, B, K, F, C | Confirmed unsupported in the reviewed native evidence. Token classes do not establish compatible request linkage or cache cause. Runtime descriptors and unproved relationship sidecars do not establish persisted delegation, controls, or resource exposure. Workspace chat remains uncharacterized.                                                                                                                                                                                                           |

M automatic remediation accepts only one named, enabled server with eligible
activity and complete observed calls. Codex resolves exactly one active trusted
project or global `mcp_servers.<name>` table. Claude resolves exactly one
standard project or global MCP source and adds only its matching deny rule to
one same-scope existing settings file. OpenCode has an exact `enabled = false`
editor, but M remains unsupported until its sources provide observed injection.
Antigravity has no public source and precedence proof for one persisted disable
field. Cursor MCP remediation is unavailable and never reads a private toggle
store or invokes a CLI command.

Cache churn selects its policy from `RepeatedContextAccounting`, not from the
agent or the session's dominant model. `CacheWrite` uses the reviewed Claude
family policy. `UncachedInput` uses the reviewed OpenAI family policy. This rule
also applies to mixed-family sessions. A cache-churn cause names a model from the
same accounting family; it does not use an unrelated dominant model.

Old-model causes remain separate by provider, API, observed model, and reviewed
replacement. Token-burn percentages are unknown when a required price or the
total-token denominator is absent. The estimator does not use a 10 percent
fallback and does not force a positive minimum.

## Passive Verification

Every exact finding with supported positive verification from a winning `Ready`
evidence publication can create one passive attempt, up to 100 attempts per
publication. Candidate filtering applies the detector, agent, source-format,
physical-target, and non-resource requirements before bounded selection. The
selection is fair across all nine detectors. V45 does not backfill old published
rows. The immutable boundary is the publication time
in milliseconds, not the session time. Replay reuses an active target. After
recurrence, a later publication can create a new attempt. An action can join an
active passive attempt, records its own action time, and preserves the passive
boundary and origin. Attempt creation, dirtying, evidence publication, and
fenced row replacement share the winning transaction.

The table below is the exact implemented positive-proof matrix. `Supported`
means the current backend can verify a fixed transition. `Unavailable` means it
does not enroll a passive attempt and cannot prove the initial fix from the
accepted passive evidence. An explicit action can store
`verificationUnavailable`. Source coverage from the main matrix still applies.

| Check | Claude Code | Codex       | OpenCode    | Pi          | Antigravity | Proof or blocker                                                                                                                                                                  |
| ----- | ----------- | ----------- | ----------- | ----------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | The finding identity is one historical session. A later session is not positive proof that the original session changed.                                                          |
| T     | Supported   | Supported   | Unavailable | Supported   | Unavailable | A complete later assessment plus an explicit same-route, same-model lower control proves the transition. Pi proves only its agent-selected policy.                                |
| S     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | A later worker or call has a different identity. No accepted source records a durable worker-setting transition.                                                                  |
| M     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed server subsets never prove that one server stopped being exposed.                                                                                                        |
| B     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Catalog-backed or observed tool subsets never prove that one definition stopped being exposed.                                                                                    |
| K     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Selected or injected document subsets never prove that one skill stopped being injected.                                                                                          |
| O     | Supported   | Supported   | Supported   | Supported   | Unavailable | The strict verifier requires actual replacement use on the same publication-attributed physical target, scope, provider, and API. Antigravity has no physical target attribution. |
| F     | Supported   | Supported   | Unavailable | Unavailable | Unavailable | A complete later assessment plus an explicit same-route, same-model standard-tier delegated request proves the transition.                                                        |
| C     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | A later request pair is not the same session-route target and does not prove a durable cache-policy transition.                                                                   |

Truncated assessment sets, sessions that start at or before the boundary,
missing controls, changed detector or catalog policy, stale projections, and
unsupported source contracts return verification unavailable or continue
watching. They never become fixed through generic absence. A fixed supported
target recurs only on a later exact positive observation. Prior contributions
end at the recurrence boundary and remain durable.

Named M/B/K resource targets are never passive verification candidates. Their
stored resource selector must be empty for every supported watch, so a resource
definition cannot use a model proof or create savings.

## Automatic Editor Support

`Auto Fix` means the backend can bind a finding to one publication-time
effective physical setting, prepare a reviewed edit, and recover an uncertain
write. `Prompt only` means the existing bounded prompt can describe the finding,
but the backend cannot prove one safe physical edit. Source versions mean the
accepted source shapes in the source inventory. No row claims every historical
agent release.

| Agent                            | Operation                   | Result                        | Reason or exact limit                                                                                                                                                                                                                                                                                                                    |
| -------------------------------- | --------------------------- | ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code                      | Model replacement           | Auto Fix                      | `ClaudeJsonl` only. The publication must bind the observed main-loop model to an existing effective `model` setting.                                                                                                                                                                                                                     |
| Claude Code                      | Reasoning effort            | Auto Fix                      | `ClaudeJsonl` T findings only. The publication must bind the observed level to an existing effective top-level `effortLevel` or model-specific `modelSettings.<model>.effortLevel`. The reviewed replacement is `medium`.                                                                                                                |
| Claude Code                      | Fast mode                   | Auto Fix                      | `ClaudeJsonl` F findings only. The finding needs explicit fast-tier evidence and the current winning existing `fastMode` value must be `true`; the editor removes that exact value. No publication-time config attribution is required.                                                                                                  |
| Claude Code                      | Named subagent model        | Auto Fix                      | `ClaudeJsonl` S findings only. The current scan must locate exactly one named Markdown agent whose frontmatter `model` equals the observed worker model.                                                                                                                                                                                 |
| Claude Code                      | MCP control                 | Prompt only                   | The source proves only an observed server subset. It does not prove full resource ownership, dependencies, or one effective local, project, plugin, managed, or connector control.                                                                                                                                                       |
| Claude Code                      | Skill control               | Auto Fix                      | A K finding must have complete invocation coverage and one exact injected skill identity. The editor accepts only one current standard `SKILL.md` winner, then writes that skill's `skillOverrides.<name> = "off"` entry.                                                                                                                |
| Codex                            | Model replacement           | Auto Fix                      | `CodexRolloutJsonl` only. The publication must bind the observed main-thread model to an existing effective top-level `model`. Project edits require an explicit `trust_level = "trusted"` entry and a repository-root cwd.                                                                                                              |
| Codex                            | Reasoning effort            | Auto Fix                      | `CodexRolloutJsonl` T findings only. The publication must bind the observed level to an existing effective top-level `model_reasoning_effort`. The reviewed replacement is `medium`.                                                                                                                                                     |
| Codex                            | Fast service tier           | Auto Fix                      | `CodexRolloutJsonl` F findings only. The finding needs explicit fast-tier evidence and the current winning existing `service_tier` must be `fast`; the editor changes it to reviewed `standard`. No publication-time config attribution is required.                                                                                     |
| Codex                            | Named subagent model        | Auto Fix                      | `CodexRolloutJsonl` S findings only. The current scan must locate exactly one named agent TOML file whose `model` equals the observed worker model.                                                                                                                                                                                      |
| Codex                            | MCP enablement              | Prompt only                   | Exact observed server exposure does not prove resource ownership, dependencies, or the effective layered `enabled` selector at publication.                                                                                                                                                                                              |
| Codex                            | Skill enablement            | Auto Fix                      | A K finding must have complete invocation coverage and one exact injected skill identity. The editor accepts only one current trusted standard `SKILL.md` winner and its matching `skills.config.<name>.enabled = false` entry.                                                                                                          |
| OpenCode                         | Model default               | Auto Fix                      | `OpenCodeJsonl` and `OpenCodeSqliteV2` O findings only. Direct `openai`, `anthropic`, and `google` provider IDs use their reviewed native API when OpenCode omits it. Publication must bind the observed `provider/model` route to the effective merged `model` value. Dynamic, remote, agent, mode, and managed overrides are rejected. |
| OpenCode                         | Named subagent model        | Auto Fix                      | S findings only. The current scan must locate exactly one named Markdown agent whose frontmatter `model` equals the observed worker model. Variant-only workers remain unavailable.                                                                                                                                                      |
| OpenCode                         | Reasoning control           | Unavailable                   | The accepted sources have no historical effort map. A variant label is not an effective reasoning control.                                                                                                                                                                                                                               |
| OpenCode                         | MCP control                 | Unavailable                   | The accepted sources have no model-facing MCP inventory or publication-time physical control attribution.                                                                                                                                                                                                                                |
| OpenCode                         | Skill control               | Auto Fix                      | A K finding must have complete invocation coverage and one exact injected skill identity. The editor derives only one current standard `SKILL.md` winner and appends one V2 `skill` deny with that exact resource.                                                                                                                       |
| Pi                               | Model and provider default  | Auto Fix                      | `PiV3Jsonl` O findings only. Publication must bind the observed `provider/model` route to an existing paired `defaultProvider` and `defaultModel` setting.                                                                                                                                                                               |
| Pi                               | Thinking level              | Auto Fix                      | `PiV3Jsonl` T findings only. Publication must bind the saved agent-selected level to an existing route-specific `modelThinkingLevels` entry or `defaultThinkingLevel`. The reviewed replacement is `medium`.                                                                                                                             |
| Pi                               | Skill exclusion             | Unavailable                   | The reviewed source does not prove the winning skill path or array precedence for a path exclusion. No `SKILL.md` file is changed.                                                                                                                                                                                                       |
| Pi, Cursor, Antigravity          | Named subagent model        | Unavailable                   | Pi extension output and Cursor or Antigravity findings do not bind one effective persisted worker-model selector.                                                                                                                                                                                                                        |
| Claude Code, Codex, OpenCode, Pi | Session compaction          | Auto Fix for D only           | The current project or global config must contain a supported disabled compaction flag or a numeric limit above the finding depth cap. The editor enables the flag or lowers that limit to the cap. Ordinary session growth, enabled controls, fixed instructions, runtime overrides, and unsupported schemas remain unavailable.        |
| Antigravity                      | Model or documented setting | Prompt only for O findings    | Accepted sources can retain direct model use, but no accepted IDE or CLI source binds it to one effective documented physical setting.                                                                                                                                                                                                   |
| Antigravity                      | MCP control                 | Unavailable                   | The reviewed native evidence has no MCP exposure or effective-control contract. IDE and CLI configuration cannot be interchanged.                                                                                                                                                                                                        |
| Claude Code                      | Built-in tool               | Auto Fix for optional B tools | One exact observed tool must have a matching canonical permission name and one existing standard settings deny list. The editor appends only that bare name. `Bash`, `Edit`, `Read`, and `Write` remain measured but cannot receive Auto Fix or a targeted disable prompt. Wildcards, scoped rules, and general permission changes are unavailable. |
| Codex                            | Built-in tool               | Unavailable                   | The documented `apps.<id>.tools.<tool>.enabled` control applies to an app tool, not one built-in tool identity.                                                                                                                                                                                                                          |
| OpenCode, Pi                     | Built-in tool               | Unavailable pending inventory | OpenCode has a V2 action-specific deny editor and Pi has a `defaultTools` list editor, but accepted sources do not prove a complete B inventory or a current effective target.                                                                                                                                                           |

The prompt matrix below comes from `remediation/prompts.rs`. A `Yes` still needs
one finding that passes the source coverage gates above.

| Agent and source                                                                                          | D   | T   | S   | M   | B   | K   | O   | F   | C   |
| --------------------------------------------------------------------------------------------------------- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Claude Code, `ClaudeJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| Codex, `CodexRolloutJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| OpenCode, `OpenCodeJsonl` or `OpenCodeSqliteV2`                                                           | Yes | No  | Yes | No  | No  | Yes | Yes | No  | Yes |
| Pi, `PiV3Jsonl`                                                                                           | Yes | Yes | Yes | No  | No  | No  | Yes | No  | Yes |
| Antigravity, `AntigravityJson`, `AntigravityBrainJsonl`, `AntigravityCascadeJson`, or `AntigravitySqlite` | Yes | No  | No  | No  | No  | No  | Yes | No  | No  |

The exact source-format Auto Fix matrix lists every `SourceFormat` once.
`Prompt only` means at least one check has prompt support but no automatic edit.

| `SourceFormat`                 | Model Auto Fix | Reasoning Auto Fix | Other remediation                           |
| ------------------------------ | -------------- | ------------------ | ------------------------------------------- |
| `ClaudeJsonl`                  | Claude Code    | Claude Code        | Auto Fix for B; prompts for D/T/S/M/K/O/F/C |
| `CodexRolloutJsonl`            | Codex          | Codex              | Prompts for D/T/S/M/B/K/O/F/C               |
| `OpenCodeJsonl`                | OpenCode       | No                 | Prompts for D/S/K/O/C                       |
| `OpenCodeSqliteV2`             | OpenCode       | No                 | Prompts for D/S/K/O/C                       |
| `PiV3Jsonl`                    | Pi             | Pi                 | Prompts for D/T/S/O/C                       |
| `CursorJsonl`                  | No             | No                 | No remediation prompt                       |
| `CursorCliAgentJsonl`          | No             | No                 | No remediation prompt                       |
| `CursorCliStoreDb`             | No             | No                 | No remediation prompt                       |
| `CursorChatStoreDb`            | No             | No                 | No remediation prompt                       |
| `CursorIdeComposer`            | No             | No                 | No remediation prompt                       |
| `CursorLegacyChatJson`         | No             | No                 | No remediation prompt                       |
| `AntigravityJson`              | No             | No                 | Prompts for D/O                             |
| `AntigravityBrainJsonl`        | No             | No                 | Prompts for D/O                             |
| `AntigravityCascadeJson`       | No             | No                 | Prompts for D/O                             |
| `AntigravityWorkspaceChatJson` | No             | No                 | No remediation prompt                       |
| `AntigravitySqlite`            | No             | No                 | Prompts for D/O                             |
| `CopilotCliJsonl`              | No             | No                 | No remediation prompt                       |
| `CopilotIdeChatJson`           | No             | No                 | No remediation prompt                       |
| `ClineSessionJson`             | No             | No                 | No remediation prompt                       |
| `ClineMessagesContractV1`      | No             | No                 | No remediation prompt                       |
| `KiroSessionJson`              | No             | No                 | No remediation prompt                       |
| `KiroChat`                     | No             | No                 | No remediation prompt                       |
| `AmpThreadJson`                | No             | No                 | No remediation prompt                       |
| `AmpFileChanges`               | No             | No                 | No remediation prompt                       |
| `WindsurfWorkspaceJson`        | No             | No                 | No remediation prompt                       |
| `WindsurfMirrorJson`           | No             | No                 | No remediation prompt                       |
| `WindsurfCascadeProtobuf`      | No             | No                 | No remediation prompt                       |
| `Uncharacterized`              | No             | No                 | No remediation prompt                       |

| Scope and environment              | macOS       | Linux       | Native Windows        | WSL         |
| ---------------------------------- | ----------- | ----------- | --------------------- | ----------- |
| Global model or reasoning setting  | Auto Fix    | Auto Fix    | Read attribution only | Unavailable |
| Project model or reasoning setting | Auto Fix    | Auto Fix    | Read attribution only | Unavailable |
| Session or worker setting          | Unavailable | Unavailable | Unavailable           | Unavailable |

The macOS and Linux implementation rejects unreviewed runtime overrides,
managed or system configuration, Codex profiles, untrusted workspaces,
unsupported precedence, missing files, duplicate definitions, malformed data,
files above 256 KiB, non-regular files, target or ancestor symlinks, and wrong
Unix owner or group. Apply re-resolves precedence, checks the original file
identity and bytes, writes an exclusive same-directory temporary file,
preserves mode and owner/group, syncs it, atomically replaces the target, syncs
the directory, and performs typed readback. A changed target conflicts without
retargeting.

Native Windows can resolve and store publication-time attribution. Apply remains
unavailable because the repository has no reviewed implementation and executable
tests for ACL preservation, reparse points, sharing conflicts, Windows file
identity, replacement semantics, and uncertain-write recovery. The backend does
not create a prepared review on Windows. WSL has a separate environment key and
never reads or edits the native host configuration.

Prepared changes are memory-bounded and expire after ten minutes. A crash in
`writing` becomes durable `recoveryNeeded`. Recovery accepts only the same
native agent, accepted source format, scope, physical target, and replacement
value after fresh override and managed-policy checks. A changed target or an
unprovable result stays in recovery and does not start a second write.

## Config Attribution Contracts

Audit date: 2026-09-14. Attribution is publication-time metadata, not
historical session evidence. The backend stores a keyed physical target hash,
scope, observed value, physical path, selector, typed expected value, and a
keyed precedence identity only after complete control observations match the
resolved setting. This local data is not sent in analytics or diagnostics.

| Agent       | Reviewed persisted contract                                                                                                                     | Attribution decision                                                                                                                                                  |
| ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code | Managed settings override CLI, local, project, and user files. `ANTHROPIC_MODEL` has per-key precedence over `model`.                           | Model and reasoning use only an existing user, project, or local selector. Managed, CLI, host, or environment winners are unavailable.                                |
| Codex       | CLI overrides trusted project files from repository root through CWD, then an explicitly selected profile, user config, and system config.      | Model and reasoning resolve every trusted nested project file. A selected profile, runtime override, system or managed config, or untrusted workspace is unavailable. |
| OpenCode    | Remote, global, custom, nested direct project, `.opencode`, inline, managed file, then MDM sources merge in that order.                         | Model attribution reads the reviewed global and nested project JSON/JSONC subset. Remote, custom, inline, managed, dynamic, agent, and mode inputs are unavailable.   |
| Pi          | Trusted project `.pi/settings.json` deep-merges over global settings. CLI provider/model/thinking and session-directory inputs take precedence. | Model and reasoning use one existing winning project or global selector. Agent-directory, CLI, or split provider/model inputs are unavailable.                        |
| Cursor      | CLI JSON, CLI permissions, MCP JSON, IDE settings, and team controls are separate contracts.                                                    | Unavailable. No accepted Cursor session source proves that one persisted CLI or IDE setting caused the observed model behavior.                                       |
| Antigravity | Documented global and workspace MCP files do not define model-setting precedence for every IDE and CLI surface.                                 | Unavailable. Accepted session sources do not bind a model or setting to one physical control.                                                                         |

The official contracts reviewed are [Claude settings][claude-config-source],
[Codex config basics][codex-config-source], [OpenCode config][opencode-config-source],
[Pi settings][pi-config-source], [Cursor CLI configuration][cursor-config-source],
and [Antigravity MCP][antigravity-config-source]. These sources describe current
configuration behavior. They do not expand accepted session-source versions.

[claude-config-source]: https://docs.anthropic.com/en/docs/claude-code/settings
[codex-config-source]: https://developers.openai.com/codex/config-basic
[opencode-config-source]: https://opencode.ai/docs/config/
[pi-config-source]: https://github.com/badlogic/pi-mono/blob/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent/docs/settings.md
[cursor-config-source]: https://cursor.com/docs/cli/reference/configuration
[antigravity-config-source]: https://antigravity.google/docs/mcp/

## Savings Contracts

All nine methods have typed inputs, methods, revisions, units, and unavailable
reasons. Known zero and negative values remain known. Missing evidence,
assumptions, comparisons, rates, revisions, or durable ownership remains
unknown. Arithmetic overflow is unknown, not a saturated saving.

| Check | Method                               | Result unit                                | Current numeric eligibility                                                                                                                                                                                                                                                                                                                                  |
| ----- | ------------------------------------ | ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| D     | Repeated context above the depth cap | Literal input tokens                       | Numeric only with an observed request total and pinned cap. Confirmed accumulation is unavailable while D verification is unavailable.                                                                                                                                                                                                                       |
| T     | Reviewed output reduction assumption | Assumed output tokens                      | Requires observed output and an explicit basis-point assumption. No default assumption exists.                                                                                                                                                                                                                                                               |
| S     | Worker model price difference        | API-equivalent USD                         | Requires exact worker tokens, reviewed alternative rates, route, pricing revision, and ownership. Missing inputs remain unknown.                                                                                                                                                                                                                             |
| M     | MCP definition exposure              | API-equivalent USD or literal input tokens | Requires attributable definition tokens and compatible-request count. Names or exposure alone are nonnumeric. Reports API-equivalent USD only when every contributing turn's model resolves in the live pricing table and the stamped pricing revision is still current; otherwise the finding still reports with literal input tokens and no dollar figure. |
| B     | Built-in definition replication      | API-equivalent USD or literal input tokens | Numeric for established catalog-backed replication counts. It is not converted into a confirmed win while B verification is unavailable. Same pricing-table and revision requirement as M applies for the API-equivalent USD figure; an unresolvable model still reports the token count.                                                                    |
| K     | Injected skill document              | API-equivalent USD or literal input tokens | Requires full document tokens and compatible-request count. Listings never qualify. Same pricing-table and revision requirement as M applies for the API-equivalent USD figure; an unresolvable model still reports the token count.                                                                                                                         |
| O     | Old-model price difference           | API-equivalent USD                         | Implemented for exact attributed Claude Code, Codex, OpenCode, and Pi replacement activity with both reviewed rates and a pricing revision. Zero and negative differences remain known.                                                                                                                                                                      |
| F     | Fast-tier price premium              | API-equivalent USD                         | Requires same-model, same-route standard and fast rates, eligible tokens, pricing revision, and ownership. Missing comparisons remain unknown.                                                                                                                                                                                                               |
| C     | Paid versus cache-read difference    | API-equivalent USD                         | Requires attributable repeated paid tokens and reviewed paid/cache rates. Raw repeated tokens alone do not establish dollars.                                                                                                                                                                                                                                |

Literal input tokens, assumed output tokens, cache-class tokens,
API-equivalent USD, and improvement counts are separate units. Aggregation adds
only values with one nonempty durable owner, no duplicate owner, and one unit.
Mixed units and unresolved overlap stay separate. Confirmed contribution rows
contain bounded derived facts, replace equal or newer facts for one owner, and
survive normal session retention. Verification transition and contribution
replacement commit together. Aggregate reads return at most 1,000 newest rows.
Durable storage keeps at most 1,000 contribution rows and 1,000 closed attempt
rows. Active attempts remain until they verify or recur.

The remediation backend lists current displayable findings even when no action
is safe. Prompt support follows the source and check limits in this document.
T and F prompt watches require fresh, complete post-boundary assessment and the
exact positive control described in the matrix before a fix can verify.
Old-model watches are stricter: only actual old or replacement model use
attributed to the same publication-time effective physical target, scope,
provider, and API can change the result. Other checks store
`verificationUnavailable`; generic absence cannot verify them. Positive-only
sources cannot verify absence, and missing later evidence remains `watching`.

Cursor can use its explicit synthesized source-header model, but does not borrow
the previous message's model. This preserves existing basic support without
claiming native per-request completeness.

## Confirmation Ledger

The maintainer confirmed these source-scoped decisions on 2026-09-08. The
alternatives below were reviewed; none authorizes runtime collection or claims
future impossibility. Workspace/unknown shapes retain `Unknown` rather than
inheriting a native format's contract.

| Date       | Agent                   | Named checks                                                                                        | Decision and alternatives reviewed                                                                                                                                                                                                                                       |
| ---------- | ----------------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 2026-09-08 | Pi                      | M (MCP), B (built-ins), K (skills), F (fast mode)                                                   | Unsupported. Core session persistence, resource/tool configuration, and official example-extension output do not supply alternative historical inventories or speed proof. See [Pi source][pi-source].                                                                   |
| 2026-09-08 | OpenCode                | M (MCP), B (built-ins), F (fast mode), T (overthinking)                                             | Unsupported. Legacy/native message schemas, CoreV2 `session_message`, and the tool registry do not save historical inventories, effective tier, or a request-resolvable effort map. See [v1.2.0 source][opencode-v1] and [CoreV2 source][opencode-core].                 |
| 2026-09-08 | Antigravity             | T (overthinking), S (subagents), M (MCP), B (built-ins), K (skills), F (fast mode), C (cache churn) | Unsupported. The admitted agy 1.0.16 `user_version = 1` subset and descriptor-backed fields provide no alternative native proof for these checks. See [adapter research][antigravity-adapter].                                                                           |
| 2026-09-14 | Cursor                  | T, S, M, B, K, F, C                                                                                 | Unsupported or unknown. The independent JSONL and `~/.cursor/chats/<workspace>/<session>/store.db` contract do not prove complete requests, effective model fallbacks, resource inventory, routes, or IDE configuration. See [Cursor chat research][cursor-chat-source]. |
| 2026-09-08 | Claude, Codex, OpenCode | M/B/K where observed evidence exists                                                                | Approved scoped observed-resource findings only. Complete observed subset plus calls is required; no session-wide clean without full inventory. Codex exact server exposure and selected documents are covered by [rollout/protocol/skills research][codex-source].      |
| 2026-09-08 | Pi                      | T, S                                                                                                | T is explicitly agent-selected policy on reviewed routes. S is limited to persisted official example-extension nested results and actual models, finding-only. See [core/session and examples/extensions/subagent][pi-source].                                           |

[opencode-v1]: https://github.com/anomalyco/opencode/tree/ffc000de8e446c63d41a2e352d119d9ff43530d0
[opencode-core]: https://github.com/anomalyco/opencode/tree/ecbc6ccac85b3e8087b6445e584318419b9e2b34
[pi-source]: https://github.com/badlogic/pi-mono/tree/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent
[codex-source]: https://github.com/openai/codex/tree/e7637306bc9246a3e42e407cb94f96b7ed345e3e
[antigravity-adapter]: https://github.com/ccusage/ccusage/blob/90e296efd1bdd25a9db07019854255284588d720/rust/adapters/antigravity/src/proto.rs
[cursor-chat-source]: https://github.com/antonvp/cursor-acp-enriched/commit/4801804543f0234bdfc266fbd53d81a6f20e9508

Research anchors include OpenCode schema/session-message and tool registry;
Pi core/session and `examples/extensions/subagent`; Codex rollout policy,
protocol models, and `ext/skills/fragments`; and ccusage
`rust/adapters/antigravity/src/proto.rs`. The [Antigravity SDK runtime schema][sdk-source] at
`52ea99480960ed02be1561f6fe57b99e7186962a` describes runtime events, not proof
that those events persist in a local session.

[sdk-source]: https://github.com/google-antigravity/antigravity-sdk-python/tree/52ea99480960ed02be1561f6fe57b99e7186962a

## Deferred Source Limits

These existing source classifications use dedicated fail-closed readers. Their
basic discovery remains supported, but none has an assessable detector-grade
contract. The matrix retains their prior source limits; `Partial` here does not
claim an implemented finding path.

| Source                             | Current boundary                                                                                                                                                                                                                                                    |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Copilot CLI                        | Persisted event logs omit per-call usage and loaded inventories that the official schema marks transient. Model/effort changes, skill calls, and subagent configuration lack a complete request-level check contract. Configuration alone is not actual delegation. |
| Copilot IDE                        | Chat JSON can carry model names, but model/time and other check facts are uncharacterized. CLI contracts do not apply to IDE storage.                                                                                                                               |
| Cline                              | Metadata/message companion loading and fingerprinting remain incomplete. Calls and model names do not prove paired timing, historical resources, or other check facts.                                                                                              |
| Kiro canonical and chat            | Separate source shapes; resource definitions, exposure, calls, models, timing, and settings lack characterized detector-grade semantics. The fallback does not inherit canonical coverage.                                                                          |
| Amp thread                         | No whole-thread check contract. Saved routing modes do not prove actual model effort or speed; native delegation, resources, timing, and accounting remain uncharacterized.                                                                                         |
| Amp file changes                   | Not a conversation session. No check can use file-change records as request, model, or inventory proof.                                                                                                                                                             |
| Windsurf workspace and mirror JSON | Calls and model names can exist, but complete resource, timing, control, and accounting semantics remain uncharacterized.                                                                                                                                           |
| Windsurf protobuf                  | Discovery recognizes Cascade paths; no bounded protobuf session parser or supported field contract exists.                                                                                                                                                          |
| Generic fallback                   | No native source contract. Recognized-looking JSON does not authorize detector-grade evidence or clean.                                                                                                                                                             |

## Coverage Promotion Rule

Change an entry to `Assessable` only when all of these conditions are true:

- The accepted source shape is explicit through a schema, header, or pinned
  producer commit and synthetic fixtures. Record a release range when known.
- The reader emits every fact required for both a finding and a clean result.
- Missing, malformed, truncated, capped, or unknown records produce partial or unavailable evidence.
- Positive, negative, and incomplete synthetic fixtures exist.
- Full and resumed reads produce equivalent evidence where resume is supported.
- The model and provider policy is reviewed where the check needs policy.
- The implementation does not use current configuration as historical session evidence.

If a passive source cannot meet these conditions, keep the entry `Partial`,
`Unsupported`, or `Unknown`. Do not convert missing evidence into a clean result.
For the reviewed targets (OpenCode, Pi, Codex, Claude Code, and Antigravity),
record alternative passive-source research and explicit maintainer confirmation
for unsupported named checks. The ledger above records the current decisions.
Cursor and other agents retain their deferred basic support; these decisions
do not assert a completed audit of all their native sources.

## Test Coverage

- Agent characterization suites in `crates/antiburn-local/tests/` cover native
  records, missing facts, scoped resources, provider controls, and malformed input.
- `resume_parity.rs`, `evidence_replay_parity.rs`, and `turn_row_replay_parity.rs`
  cover supported resume and persisted-row paths. Desktop
  `analysis/tests/claude_parent_child.rs` and scan tests cover Claude sidecar joins
  and change detection.

The matrix is manually reviewed; the inventory test does not generate or prove
every cell. Unknown changed evidence-bearing shapes must make affected evidence
partial or unavailable, not pass through a generic reader as clean.
