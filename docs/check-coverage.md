# Burn Check Source Coverage

Audit date: 2026-09-09.

This document covers local passive evidence only. Coverage must not use hooks,
new extensions, runtime subscriptions, or agent calls to fill evidence gaps. Existing
persisted output from the reviewed Pi example extension is a passive input. The
current OpenCode WSL discovery conflict is recorded in `session-coverage.md`.

See [`session-coverage.md`](session-coverage.md) for discovery, framing, parsing,
companion-source, and provider-route coverage for the same source formats.

## Status Rules

| Status | Meaning |
| --- | --- |
| Assessable | The current reader can produce the evidence needed for a finding and a clean result. A damaged or incomplete session can still be partial. |
| Partial | The source has useful passive evidence, but the current reader or the source cannot prove all required facts. Do not report a clean result. |
| Unsupported | The reviewed passive sources do not prove a required fact or its policy semantics. This is source-scoped, not a claim about future formats. |
| Unknown | The source or its relevant field semantics are not characterized. Do not infer support from a path, field name, mode name, or generic JSON shape. |

`Partial` can mean finding-only support or an unimplemented evidence path. The
limits below distinguish them. `Assessable` describes the accepted source
contract, not every session or historical release. Complete session facts,
eligible activity, and reviewed model/provider policy remain necessary for clean.

## Checks

| Code | Check |
| --- | --- |
| D | Session overdepth |
| T | Model overthinking |
| S | Overpowered subagents |
| M | Unused MCP servers |
| B | Unused built-in tools |
| K | Unused skills |
| O | Old model usage |
| F | Fast mode overuse |
| C | Cache churn |

## Source Inventory

The tables list all 26 `SourceFormat` keys. Known source shape and release
version are separate facts. A version range is not always available; an accepted
schema, header, or pinned producer commit with synthetic fixtures can establish
a bounded contract. No row promises parity across all historical versions.

| `SourceFormat` | Passive source format | Version statement | Current reader |
| --- | --- | --- | --- |
| `ClaudeJsonl` | Claude Code session JSONL and child sidecars | Accepted persisted shapes have synthetic fixtures; no universal release range | Dedicated |
| `CodexRolloutJsonl` | Codex rollout JSONL, with discovered child rollouts | Accepted rollout/protocol shapes and pinned producer research below; synthetic fixtures | Dedicated |
| `OpenCodeJsonl` | OpenCode legacy exported session data | Accepted export wrappers and native message/part shapes; pinned research below | Dedicated |
| `OpenCodeSqliteV2` | OpenCode SQLite `session`, `message`, `part` tables | Fixture-backed table contract; not CoreV2 `session_message` | Dedicated |
| `PiV3Jsonl` | Pi session JSONL | Header version 3 and pinned core/example-extension shapes; synthetic fixtures | Dedicated |
| `CursorJsonl` | Cursor compatibility JSONL without a surface marker | Unversioned and uncharacterized | Dedicated shared Cursor reader |
| `CursorCliAgentJsonl` | Cursor agent transcript JSONL | Unversioned; current synthesis is partial | Dedicated shared Cursor reader |
| `CursorCliStoreDb` | Cursor CLI `chats/**/store.db` data | Private and unversioned; current synthesis is partial | Dedicated shared Cursor reader |
| `CursorIdeComposer` | Cursor IDE composer data from `state.vscdb` | Private and unversioned; current synthesis is partial | Dedicated shared Cursor reader |
| `CursorLegacyChatJson` | Cursor IDE `chatSessions/*.json` | Unversioned and uncharacterized | Dedicated fail-closed profile |
| `AntigravityJson` | Internal Antigravity compatibility profile | Not emitted by current source classification | Dedicated shared profile |
| `AntigravityBrainJsonl` | Antigravity brain transcript JSONL | Unversioned; current shape is partially characterized | Dedicated |
| `AntigravityCascadeJson` | Antigravity API cascade or mirror JSON | Unversioned; current shape is partially characterized | Dedicated |
| `AntigravityWorkspaceChatJson` | Antigravity workspace `chatSessions/*.json` | Unversioned and uncharacterized | Dedicated fail-closed profile |
| `AntigravitySqlite` | Native `conversations/<uuid>.db` plus an optional brain transcript | Private descriptor subset; installed 2.11.0 research below, not full schema support | Dedicated |
| `CopilotCliJsonl` | `session-state/<id>/events.jsonl` | Copilot CLI GA 2026 shape; no exact schema revision is pinned | Dedicated fail-closed |
| `CopilotIdeChatJson` | VS Code-family `chatSessions/*.json` | Unversioned; IDE and CLI contracts are separate | Dedicated fail-closed |
| `ClineSessionJson` | Cline metadata and message companion | Cline 2.0+ naming is known; message schemas are not pinned | Dedicated fail-closed |
| `KiroSessionJson` | Kiro workspace-session JSON | Unversioned and uncharacterized | Dedicated fail-closed |
| `KiroChat` | Kiro `.chat` fallback | Unversioned and uncharacterized | Dedicated fail-closed |
| `AmpThreadJson` | Amp `threads/*.json` whole-thread record | Unversioned and uncharacterized | Dedicated fail-closed |
| `AmpFileChanges` | Amp `file-changes/**/*.{json,jsonl}` | File-change fallback, not a thread | Dedicated fail-closed |
| `WindsurfWorkspaceJson` | Windsurf workspace chat JSON | Unversioned and uncharacterized | Dedicated fail-closed |
| `WindsurfMirrorJson` | Configured Windsurf mirror JSON | Unversioned and uncharacterized | Dedicated fail-closed |
| `WindsurfCascadeProtobuf` | Windsurf Cascade `.pb` data | Private and uncharacterized | Dedicated fail-closed when discovered |
| `Uncharacterized` | Unknown-agent generic fallback | No source contract | Generic fail-closed |

## Coverage Matrix

This manual matrix records implemented eligibility and audited source limits,
not just binary capability flags. The inventory test checks keys and cell
vocabulary; behavior tests separately check finding and clean gates.

| `SourceFormat` | D | T | S | M | B | K | O | F | C |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ClaudeJsonl` | Assessable | Assessable | Assessable | Partial | Partial | Partial | Assessable | Assessable | Assessable |
| `CodexRolloutJsonl` | Assessable | Assessable | Assessable | Partial | Partial | Partial | Assessable | Assessable | Assessable |
| `OpenCodeJsonl` | Assessable | Unsupported | Assessable | Unsupported | Unsupported | Partial | Assessable | Unsupported | Assessable |
| `OpenCodeSqliteV2` | Assessable | Unsupported | Assessable | Unsupported | Unsupported | Partial | Assessable | Unsupported | Assessable |
| `PiV3Jsonl` | Assessable | Assessable | Partial | Unsupported | Unsupported | Unsupported | Assessable | Unsupported | Assessable |
| `CursorJsonl` | Unsupported | Unknown | Unknown | Unknown | Unknown | Unknown | Partial | Unknown | Unknown |
| `CursorCliAgentJsonl` | Unsupported | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `CursorCliStoreDb` | Unsupported | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `CursorIdeComposer` | Unsupported | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `CursorLegacyChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| `AntigravityJson` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported |
| `AntigravityBrainJsonl` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported |
| `AntigravityCascadeJson` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported |
| `AntigravityWorkspaceChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| `AntigravitySqlite` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported |
| `CopilotCliJsonl` | Unsupported | Partial | Partial | Partial | Unsupported | Partial | Partial | Unknown | Unsupported |
| `CopilotIdeChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Partial | Unknown | Unknown |
| `ClineSessionJson` | Unknown | Unknown | Unknown | Partial | Partial | Unknown | Partial | Unknown | Unknown |
| `KiroSessionJson` | Unknown | Unknown | Unknown | Partial | Partial | Partial | Partial | Unknown | Unknown |
| `KiroChat` | Unknown | Unknown | Unknown | Partial | Partial | Unknown | Partial | Unknown | Unknown |
| `AmpThreadJson` | Partial | Partial | Partial | Unknown | Partial | Unknown | Partial | Unknown | Unknown |
| `AmpFileChanges` | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `WindsurfWorkspaceJson` | Unknown | Unknown | Unknown | Partial | Partial | Unknown | Partial | Unknown | Unknown |
| `WindsurfMirrorJson` | Unknown | Unknown | Unknown | Partial | Partial | Unknown | Partial | Unknown | Unknown |
| `WindsurfCascadeProtobuf` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| `Uncharacterized` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |

## Evidence Boundaries

No current reader proves a full historical resource inventory. All M/B/K checks
deny session-wide `Clean`, even when a nested observed-resource map is complete.
A scoped finding requires complete coverage of that observed subset, calls, and
eligible activity. An unrelated partial resource group does not block it.

Thread attribution retains at most 512 distinct UUIDs, each at most 256 bytes.
An overflow or oversized UUID records `CapExceeded`, makes attribution
incomplete, and blocks every clean result that needs complete affected evidence.
An oversized resume identity set is rejected instead of being trusted.

Skills mean full documents injected into model context. Listings, installed
skills, and names in tool calls do not prove unused document overhead. Resource
identity is retained without copying private document bodies into evidence.

| Source | Checks | Implemented contract and remaining limit |
| --- | --- | --- |
| Claude and Codex | D, O | Direct request depth and timed model use reach checks independently of token-accounting policy. Clean needs all required session facts and reviewed model state. Unknown models are not automatically current. |
| Claude and Codex | T, F | Request-level model, effort, speed, and route observations are evaluated together. Codex preserves explicit provider/control changes and inherited fork state; child controls retain delegated scope. Missing eligible signals or unreviewed routes deny clean. |
| Claude | S | Exact `Task`/`Agent` call IDs join unique sidecar `toolUseId` claims to actual child models and the model on the parent call. Missing, duplicate, malformed, mismatched, or nested-parent claims remain partial. Requested model aliases and directory ancestry are not proof. |
| Codex | S | Owned `spawn_agent` records and discovered child rollouts provide delegation and actual models. Incomplete child evidence cannot prove clean. |
| Claude | M, K | Observed MCP injection plus exact server calls and full skill-document injection plus invocation identity support scoped findings. Skill listings alone do not. The observed subset is not a historical inventory. |
| Codex | M | Accepted completed client `tool_search_output` namespace records expose exact MCP server identities. Complete observed exposure and calls support scoped findings without a full-inventory capability. Ambiguous or incomplete search results do not establish injection. |
| Claude and Codex | B | The existing harness-version/model catalog path supports scoped definition findings with complete calls. Deferred, situational, and zero-cost definitions are excluded. Catalog resolution does not prove every historical enabled tool or exposure change; it cannot justify a whole-inventory claim. |
| Codex | K | Selected full skill documents reach observed injection/invocation evidence. Listings remain availability only. Selected documents do not establish unused listing overhead or full inventory coverage. |
| OpenCode | S | Native `task` metadata identifies the child session and model; ancestry and the child's assistant model must agree. The parent model comes from the task request. A bare `subtask`, fork, or `parent_id` relation is insufficient. |
| OpenCode | K | Complete native selected-skill results preserve full identity as injected and invoked. Truncated, compacted, empty, or invalid result wrappers do not prove full injection. This is observed selected-skill support, not an unused-listing finding or complete inventory. |
| OpenCode | T, M, B, F | Confirmed unsupported for the reviewed sources: no historical effort map, model-facing resource inventories, or effective speed tier. Variant labels, current configuration, and tool registries cannot substitute. |
| Pi | T | `EffortSemantics::AgentSelectedPolicy` evaluates the saved agent-selected thinking level, not translated provider effort. Reviewed native routes and branch/fork policy state are retained. Missing levels/routes and unknown models fail closed; provider overrides are not guessed. |
| Pi | S | Existing output from the official subagent example extension supplies nested `toolResult` messages, exact native call/worker identity, and actual models. This is finding-only. Arbitrary extensions, fork ancestry, requested aliases, and a nonpremium observed worker cannot establish clean. |
| Pi | M, B, K, F | Confirmed unsupported for the reviewed sources. Tool calls and bounded skill invocation identity do not establish historical resource exposure or speed. No alternative local proof was identified. |
| Claude, Codex, OpenCode, Pi | C | Durable request provider/API fields and the compatible-request query select reviewed cache-write or uncached-input accounting. Main-thread identity, order, token classes, model, route, and compaction boundaries constrain pairs. Unknown or incompatible segments remain partial, not clean. Google cache policy remains unreviewed. |
| OpenCode | C | Both accepted export and SQLite shapes use validated ordered history. `parentID` identifies the user being answered, not the predecessor. Missing wrappers/timestamps, duplicate or out-of-order messages, and unresolved forks prevent complete history. CoreV2 `session_message` is not the existing SQLite table contract. |
| Cursor | D, O | O retains direct timed-model findings; the source gate denies clean on every surface. D remains unavailable because the current reader does not emit request-usage evidence. Synthetic source-gate tests do not establish native parsing support. |
| Cursor | T, S, M, B, K, F, C | Broader surface characterization is deferred. Current settings, relations, inventories, and cache evidence remain partial, unknown, or unsupported as listed; no new parity claim is made. |
| Antigravity | D, O | Brain/cascade steps and native SQLite preserve direct usage/model findings where present. Missing model/time is not filled from an earlier step or an invented database timestamp. Private identity, enum, and completeness gaps deny clean. |
| Antigravity | T, S, M, B, K, F, C | Confirmed unsupported in the reviewed native evidence. Token classes do not establish compatible request linkage or cache cause. Runtime descriptors and unproved relationship sidecars do not establish persisted delegation, controls, or resource exposure. Workspace chat remains uncharacterized. |

Cache churn selects its policy from `RepeatedContextAccounting`, not from the
agent or the session's dominant model. `CacheWrite` uses the reviewed Claude
family policy. `UncachedInput` uses the reviewed OpenAI family policy. This rule
also applies to mixed-family sessions. A cache-churn cause names a model from the
same accounting family; it does not use an unrelated dominant model.

Old-model causes remain separate by provider, API, observed model, and reviewed
replacement. Token-burn percentages are unknown when a required price or the
total-token denominator is absent. The estimator does not use a 10 percent
fallback and does not force a positive minimum.

The remediation backend lists only findings that can produce a safe bounded
prompt. Prompt support follows the source and check limits in this document.
Generic prompt watches require fresh, complete post-boundary assessment before
absence can verify a fix. Old-model watches are stricter: only actual old or
replacement model use attributed to the same publication-time effective
physical target, scope, provider, and API can change the result. Positive-only
sources cannot verify absence, and missing later evidence remains `watching`.

Cursor can use its explicit synthesized source-header model, but does not borrow
the previous message's model. This preserves existing basic support without
claiming native per-request completeness.

## Confirmation Ledger

The maintainer confirmed these source-scoped decisions on 2026-09-08. The
alternatives below were reviewed; none authorizes runtime collection or claims
future impossibility. Workspace/unknown shapes retain `Unknown` rather than
inheriting a native format's contract.

| Date | Agent | Named checks | Decision and alternatives reviewed |
| --- | --- | --- | --- |
| 2026-09-08 | Pi | M (MCP), B (built-ins), K (skills), F (fast mode) | Unsupported. Core session persistence, resource/tool configuration, and official example-extension output do not supply alternative historical inventories or speed proof. See [Pi source][pi-source]. |
| 2026-09-08 | OpenCode | M (MCP), B (built-ins), F (fast mode), T (overthinking) | Unsupported. Legacy/native message schemas, CoreV2 `session_message`, and the tool registry do not save historical inventories, effective tier, or a request-resolvable effort map. See [v1.2.0 source][opencode-v1] and [CoreV2 source][opencode-core]. |
| 2026-09-08 | Antigravity | T (overthinking), S (subagents), M (MCP), B (built-ins), K (skills), F (fast mode), C (cache churn) | Unsupported. Installed 2.11.0 descriptors, native conversation databases, brain/cascade data, and adapter protobuf research provide no alternative native proof for these checks. See [adapter research][antigravity-adapter]. |
| 2026-09-08 | Claude, Codex, OpenCode | M/B/K where observed evidence exists | Approved scoped observed-resource findings only. Complete observed subset plus calls is required; no session-wide clean without full inventory. Codex exact server exposure and selected documents are covered by [rollout/protocol/skills research][codex-source]. |
| 2026-09-08 | Pi | T, S | T is explicitly agent-selected policy on reviewed routes. S is limited to persisted official example-extension nested results and actual models, finding-only. See [core/session and examples/extensions/subagent][pi-source]. |

[opencode-v1]: https://github.com/anomalyco/opencode/tree/ffc000de8e446c63d41a2e352d119d9ff43530d0
[opencode-core]: https://github.com/anomalyco/opencode/tree/ecbc6ccac85b3e8087b6445e584318419b9e2b34
[pi-source]: https://github.com/badlogic/pi-mono/tree/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent
[codex-source]: https://github.com/openai/codex/tree/e7637306bc9246a3e42e407cb94f96b7ed345e3e
[antigravity-adapter]: https://github.com/ccusage/ccusage/blob/90e296efd1bdd25a9db07019854255284588d720/rust/adapters/antigravity/src/proto.rs

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

| Source | Current boundary |
| --- | --- |
| Copilot CLI | Persisted event logs omit per-call usage and loaded inventories that the official schema marks transient. Model/effort changes, skill calls, and subagent configuration lack a complete request-level check contract. Configuration alone is not actual delegation. |
| Copilot IDE | Chat JSON can carry model names, but model/time and other check facts are uncharacterized. CLI contracts do not apply to IDE storage. |
| Cline | Metadata/message companion loading and fingerprinting remain incomplete. Calls and model names do not prove paired timing, historical resources, or other check facts. |
| Kiro canonical and chat | Separate source shapes; resource definitions, exposure, calls, models, timing, and settings lack characterized detector-grade semantics. The fallback does not inherit canonical coverage. |
| Amp thread | No whole-thread check contract. Saved routing modes do not prove actual model effort or speed; native delegation, resources, timing, and accounting remain uncharacterized. |
| Amp file changes | Not a conversation session. No check can use file-change records as request, model, or inventory proof. |
| Windsurf workspace and mirror JSON | Calls and model names can exist, but complete resource, timing, control, and accounting semantics remain uncharacterized. |
| Windsurf protobuf | Discovery recognizes Cascade paths; no bounded protobuf session parser or supported field contract exists. |
| Generic fallback | No native source contract. Recognized-looking JSON does not authorize detector-grade evidence or clean. |

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

## Contract Tests

- [`check_coverage_contract.rs`](../crates/antiburn-local/tests/check_coverage_contract.rs)
  checks every enum key once in each inventory, the nine detector columns, valid
  status words, partial-fact clean denial, source gates, and direct D/O findings.
- Agent characterization suites in `crates/antiburn-local/tests/` cover native
  records, missing facts, scoped resources, provider controls, and malformed input.
- `resume_parity.rs`, `evidence_replay_parity.rs`, and `turn_row_replay_parity.rs`
  cover supported resume and persisted-row paths. Desktop
  `analysis/tests/claude_parent_child.rs` and scan tests cover Claude sidecar joins
  and change detection.

The matrix is manually reviewed; the inventory test does not generate or prove
every cell. Unknown changed evidence-bearing shapes must make affected evidence
partial or unavailable, not pass through a generic reader as clean.
