# Session Parsing Coverage

Audit date: 2026-09-16.

This document records how Antiburn discovers and parses local session sources.
It covers source identity, framing, companion data, normalized facts, and
provider-route extraction. See [`check-coverage.md`](check-coverage.md) for the
nine burn checks that can use those facts.

This is a living contract. A discovered path does not prove that its contents
are understood. A parsed field can support a scoped result without proving full
historical coverage.

## Status Rules

| Status          | Meaning                                                                                                         |
| --------------- | --------------------------------------------------------------------------------------------------------------- |
| Characterized   | Committed fixtures define the accepted source shape and important failure cases.                                |
| Partial         | The reader parses useful facts, but source shapes, versions, companions, or completeness rules still have gaps. |
| Uncharacterized | Discovery can identify the source, but no detector-grade parsing contract exists. The reader must fail closed.  |
| Not a session   | The discovered data does not contain a conversation session and must not inherit session coverage.              |

## Pipeline Contract

Every supported source passes through these boundaries:

1. Discovery identifies the agent, native source, surface, session identity, and companions.
2. Source validation pins the accepted file boundary or database snapshot.
3. The shell supplies `SessionInput.source_format`; `reader_for` selects the
   `SessionReader` by agent label, and the reader consumes that source contract.
4. Bounded framing rejects oversized, malformed, truncated, or unreadable records.
5. Parsing emits normalized metrics, content, and evidence observations.
6. The evidence sink records complete, partial, or unavailable facts.
7. The check contract decides whether a finding or clean result is valid.

Unknown changed evidence-bearing shapes must not fall through to a generic
interpretation that can produce clean. A known shape need not have a universal
release range: an accepted schema, header, or pinned producer commit with
synthetic fixtures can define its contract. This does not prove all historical
versions. Full and resumed reads must agree where resume is supported.

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

Codex `thread_rolled_back` is an explicit request-history boundary. It is retained
as a compaction-boundary event so depth, cache accounting, metrics, and resumed
reads cannot join requests across the rollback.

Maintainer confirmation (2026-09-12): repair delayed exact-copy deduplication,
request pairing, and full-denominator accounting. Reviewed passive alternatives
include increasing the time limit and adjusting thresholds; neither repairs
all three accounting errors. Per-session thresholds remain unchanged.

Claude JSONL usage parses a nested `cache_creation` breakdown
(`ephemeral_1h_input_tokens`, `ephemeral_5m_input_tokens`) into
`cache_write_1h_tokens`, the subset of cache-creation tokens Anthropic bills
at the one-hour premium rate instead of the catalogue's default (five-minute)
rate. The flat `cache_creation_input_tokens` total takes the larger of itself
and the breakdown's sum; the one-hour count never exceeds that total. A
present breakdown always wins. Claude Code has run with one-hour caching
configured throughout, so a Claude record with no nested breakdown
classifies its whole cache-creation total as one-hour writes instead,
mirroring the cadence parser's own default for sessions that predate the
breakdown. Non-Claude sources carry no such default: an absent breakdown
there reports zero one-hour tokens.

Inline materialized sources use a fingerprint of the full bounded content, not
only a head region. The content is already materialized and size-bounded before
this fingerprint is calculated. OpenCode SQLite fingerprints stream every
selected value from the accepted root and descendant `session`, `message`, and
`part` cluster in stable table and row order. This detects a content change even
when row counts and saved timestamps do not change.

## Review Scope

The reviewed targets are OpenCode, Pi, Codex, Claude Code, and Antigravity.
Their accepted source contracts and exact unsupported checks are recorded here
and in the [confirmation ledger](check-coverage.md#confirmation-ledger).
Cursor and other agents retain basic current support with broader work deferred.
Dedicated reader registration alone does not establish usable session analysis.

## Source Matrix

The table lists all 32 `SourceFormat` names from
`crates/antiburn-local/src/analysis/evidence.rs`, each exactly once.

Before a production session enters the local index, its CWD must resolve to a
Git repository. The scan maps linked worktrees to the canonical main root and
rejects missing or unresolved CWDs. A disabled repository is rejected when
either its CWD or its canonical root is in the existing ignored-path set.
Newly discovered repositories remain enabled by default.

| `SourceFormat`                 | Agent         | Native source                                                                                                           | Discovery and framing                                                                                                                                                                                                                                                       | Parsed facts                                                                                                                                                                                                                                                                            | State                                                                                                                   |
| ------------------------------ | ------------- | ----------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `ClaudeJsonl`                  | Claude Code   | `~/.claude/projects/<workspace>/*.jsonl`                                                                                | Native discovery; bounded JSONL with source claims; resume supported; the reviewed 2.1.220-2.1.246 sidecar contract uses a unique `toolUseId` join                                                                                                                          | Usage (including the nested one-hour/five-minute cache-write split), token classes, time, models, request controls/routes, calls, observed resource injection, thread links, compactions, exact Task/Agent child pairing, quota and provider incidents from `isApiErrorMessage` records | Characterized accepted core; transcript format is private, so unknown evidence-bearing records deny clean               |
| `CodexRolloutJsonl`            | Codex         | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`                                                                          | Native discovery with child rollouts; bounded JSONL; resume supported; recorder source pin defines `session_meta`, `turn_context`, `event_msg`, `response_item`, ordinal, and `compacted` rows                                                                              | Per-response usage and context window, time, models, provider/control inheritance, service tier, tools, harness version, spawn records, selected skill documents, exact tool-search MCP exposure, compactions, quota and provider incidents from `task_complete` errors                 | Characterized accepted core; resource subsets only                                                                      |
| `OpenCodeJsonl`                | OpenCode      | Legacy exported session JSONL                                                                                           | Native persisted export or WSL CLI export; bounded JSONL with validated history wrappers/order                                                                                                                                                                              | Usage, time, models, provider/API fields where saved, task proof, selected skills, tools, compactions, session/message identities                                                                                                                                                       | Characterized accepted export; WSL is not disk-only; variant labels do not prove effort or speed; no resource inventory |
| `OpenCodeSqliteV2`             | OpenCode      | `~/.local/share/opencode/opencode.db` or platform equivalent                                                            | Read-only transaction snapshot, including visible WAL rows; requires `session(id,time_created)`, `message(id,session_id,data)`, and `part(message_id,data)`; row-streamed content fingerprint; validated creation-time/message-ID order                                     | Native messages and parts, task metadata joined to child models, selected skills, usage, provider/API fields, compactions, identities                                                                                                                                                   | Characterized table contract; not CoreV2 `session_message`                                                              |
| `PiV3Jsonl`                    | Pi            | `~/.pi/agent/sessions/**/*.jsonl` or `PI_AGENT_DIR`                                                                     | Native discovery; version 3 header; bounded JSONL; resume supported                                                                                                                                                                                                         | Usage at the nested request-start timestamp, top-level event time, provider/API/model, agent-selected thinking policy, branch/fork state, tools, links, compactions, official example-extension nested worker results                                                                   | Characterized core; extension delegation is finding-only, not arbitrary extension support                               |
| `CursorJsonl`                  | Cursor        | In-memory or compatibility JSONL without a source marker                                                                | Dedicated reader with bounded JSONL; native surface is unknown                                                                                                                                                                                                              | Generic Cursor role, content, timestamp, model, tool call, and record ID fields                                                                                                                                                                                                         | Uncharacterized compatibility format                                                                                    |
| `CursorCliAgentJsonl`          | Cursor        | `.cursor/projects/*/agent-transcripts/**` with chat metadata                                                            | Native discovery; transcript and metadata synthesis; bounded JSONL reader; the JSONL export is independent from the store contract                                                                                                                                          | Role, content, timestamps, models, tool calls, and selected record IDs                                                                                                                                                                                                                  | Partial; no model fallback or configuration inference                                                                   |
| `CursorCliStoreDb`             | Cursor        | Legacy Cursor CLI `chats/**/store.db`                                                                                   | Read-only database extraction into marked JSONL; reviewed `blobs(id,data)` and `meta(key,value)` subset                                                                                                                                                                     | Scalar messages, title, workspace, timestamps, model, IDs, and fork-prefix hints                                                                                                                                                                                                        | Partial; structured records are reduced during synthesis                                                                |
| `CursorChatStoreDb`            | Cursor        | Cursor chat `~/.cursor/chats/<workspace>/<session>/store.db`                                                            | Read-only database extraction; same reviewed `blobs(id,data)` and `meta(key,value)` subset, separate path contract                                                                                                                                                          | No detector-grade fact contract beyond direct timestamped model observations                                                                                                                                                                                                            | Partial; chat persistence does not establish effective model, inventory, route, or IDE configuration                    |
| `CursorIdeComposer`            | Cursor        | Workspace and global `state.vscdb` composer data                                                                        | Paired database discovery and synthesis into marked JSONL                                                                                                                                                                                                                   | Composer identity, title, workspace, timestamps, model, messages, bubble IDs, and an `isSubagent` hint                                                                                                                                                                                  | Partial; structured calls and relations are reduced during synthesis                                                    |
| `CursorLegacyChatJson`         | Cursor        | VS Code-family `chatSessions/*.json`                                                                                    | Native file discovery; dedicated fail-closed profile                                                                                                                                                                                                                        | No detector-grade fact contract                                                                                                                                                                                                                                                         | Uncharacterized                                                                                                         |
| `AntigravityJson`              | Antigravity   | Internal compatibility profile                                                                                          | Not emitted by current source classification                                                                                                                                                                                                                                | Shared partial Antigravity JSON facts                                                                                                                                                                                                                                                   | Internal profile; not a native source                                                                                   |
| `AntigravityBrainJsonl`        | Antigravity   | Brain transcript JSONL from CLI, IDE 2.0, or legacy paths                                                               | Native file discovery; bounded JSONL                                                                                                                                                                                                                                        | Step usage where present, timestamps, direct models, tool calls, and selected tool input                                                                                                                                                                                                | Partial                                                                                                                 |
| `AntigravityCascadeJson`       | Antigravity   | API cascade or configured mirror JSON                                                                                   | Native or configured file discovery; bounded whole-document parsing                                                                                                                                                                                                         | Nested steps, usage, timestamps, direct models, tool calls, and selected arguments                                                                                                                                                                                                      | Partial                                                                                                                 |
| `AntigravityWorkspaceChatJson` | Antigravity   | Workspace `chatSessions/*.json`                                                                                         | Native file discovery; dedicated fail-closed profile                                                                                                                                                                                                                        | No detector-grade fact contract                                                                                                                                                                                                                                                         | Uncharacterized                                                                                                         |
| `AntigravitySqlite`            | Antigravity   | Native `conversations/<uuid>.db` with an optional sibling brain transcript                                              | Read-only transaction snapshot, including visible WAL rows; requires `PRAGMA user_version = 1`, reviewed `gen_metadata(idx,data)` or `steps(idx,metadata)` columns, and a private protobuf subset; companion fingerprinting                                                 | Generation and step usage, retries, token classes, direct timestamps/model strings, companion tool rows; bounded joins retain exact response identities and conflicting model joins are partial                                                                                         | Partial; missing model/time stays missing; identity, enums, routes, and linkage remain incomplete                       |
| `CopilotCliJsonl`              | Copilot       | `~/.copilot/session-state/<uuid>/events.jsonl` plus sibling `session-store.db`                                          | Native CLI discovery; bounded JSONL; strict public v1 envelope, typed event graph, and schema-v7 read-only request store; source changes reject publication                                                                                                                 | Shutdown model usage, selected model changes, request usage, and started/completed or failed subagent model relations; prompts, content, tool arguments, and results are not read                                                                                                       | Characterized v1 event and schema-v7 bundle contract; no inventory, speed, request-depth, or cache-churn evidence       |
| `CopilotIdeChatJson`           | Copilot       | VS Code-family `chatSessions/*.json`                                                                                    | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                         | No IDE-specific fact contract                                                                                                                                                                                                                                                           | Uncharacterized                                                                                                         |
| `ClineSessionJson`             | Cline         | Legacy Cline metadata JSON and message companion                                                                        | Metadata-only legacy source; message schemas are not pinned and the companion is not loaded as one analysis source                                                                                                                                                          | No paired detector-grade fact contract                                                                                                                                                                                                                                                  | Uncharacterized; fail-closed                                                                                            |
| `ClineMessagesContractV1`      | Cline         | `.cline/data/db/sessions.db` with root manifest and canonical message artifacts under `data/tasks/` or `data/sessions/` | Read-only SQLite snapshot includes WAL rows; requires exact `sessions` columns including `agent_id`, terminal root/child rows, matching root manifest, canonical agent-named paths, and child origin joins                                                                  | Terminal assistant timestamps/models/token classes, tool names, direct root/child scope, and child models; message text, prompts, tool payloads, results, paths, and secrets are discarded                                                                                              | Characterized v1 bundle; S/O findings only; no clean, request depth, inventory, effort, speed, or cache claim           |
| `KiroSessionJson`              | Kiro          | Canonical workspace-session JSON                                                                                        | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                         | No canonical detector-grade fact contract                                                                                                                                                                                                                                               | Uncharacterized                                                                                                         |
| `KiroChat`                     | Kiro          | `.chat` fallback                                                                                                        | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                         | No fallback detector-grade fact contract                                                                                                                                                                                                                                                | Uncharacterized                                                                                                         |
| `KiroCliV2Bundle`              | Kiro CLI V2   | `~/.kiro/sessions/cli/<uuid>.json` plus matching `.jsonl`                                                               | Both exact UUID siblings are required. Metadata requires `session_state.version = "v1"`; journal requires only V1 `Prompt`, `AssistantMessage`, and `ToolResults` envelopes. `.history` is ignored; `.lock` is liveness only.                                               | Model identity, safe token fields, tool names, and a child `parent_session_id`; no prompt, message, tool payload, path, or permission retention.                                                                                                                                        | Characterized V2 fixture contract; D and S are unavailable, C is unsupported, and clean is disabled.                    |
| `KiroCliV3Bundle`              | Kiro CLI V3   | `~/.kiro/sessions/<workspace>/sess_<uuid>/session.json` plus `messages.jsonl`                                           | Separate directory discovery requires both files; both files are included in one source fingerprint. The producer has not published a stable `session.json` contract, so parsing fails closed.                                                                              | No detector-grade fact contract                                                                                                                                                                                                                                                         | Uncharacterized and fail-closed                                                                                         |
| `KiroChatSaveExport`           | Kiro CLI      | Manual `/chat save` JSON export                                                                                         | Public docs confirm a user-chosen JSON export path but do not define its JSON schema. It is not scanned or parsed.                                                                                                                                                          | No detector-grade fact contract                                                                                                                                                                                                                                                         | Unsupported manual export shape                                                                                         |
| `AmpThreadJson`                | Amp           | `threads/*.json`                                                                                                        | Explicit full-export JSON only; requires envelope version 39, matching `threadId`, ordered messages, and bounded field sizes; file-change artifacts are separate                                                                                                            | Direct assistant usage, total/max input context, model, timestamp, tool names, and activated skill identities; no child-model inference or cache-churn claim                                                                                                                            | Characterized v39 export; D/O findings only; no clean result and no S/C support                                         |
| `AmpFileChanges`               | Amp           | `file-changes/**/*.{json,jsonl}`                                                                                        | Native fallback discovery                                                                                                                                                                                                                                                   | File changes only                                                                                                                                                                                                                                                                       | Not a session                                                                                                           |
| `WindsurfWorkspaceJson`        | Windsurf      | Workspace chat JSON                                                                                                     | Native file discovery; dedicated fail-closed reader                                                                                                                                                                                                                         | No workspace detector-grade fact contract                                                                                                                                                                                                                                               | Uncharacterized                                                                                                         |
| `WindsurfMirrorJson`           | Windsurf      | Configured mirror JSON                                                                                                  | Configured file discovery; dedicated fail-closed reader                                                                                                                                                                                                                     | No mirror detector-grade fact contract                                                                                                                                                                                                                                                  | Uncharacterized                                                                                                         |
| `WindsurfCascadeProtobuf`      | Windsurf      | Cascade `.pb` data                                                                                                      | Discovery walks `~/.codeium/windsurf/cascade`; no bounded protobuf session parser                                                                                                                                                                                           | No parsed session facts                                                                                                                                                                                                                                                                 | Uncharacterized                                                                                                         |
| `DevinLocalSqlite`             | Devin         | `~/.local/share/devin/cli/sessions.db`                                                                                  | Read-only WAL-visible transaction; requires migration 17 and the reviewed `sessions`, `message_nodes`, `subagent_heads`, and `tool_call_state` columns; one source per `sessions.id`; active path follows `main_chain_id`; freshness fingerprints include all reader inputs | Timestamped messages, models, deduplicated tool calls, and exact `run_subagent` child relations when child agent ID, child chain node, and actual child model agree; ACP schema 6 is optional child-only context                                                                        | Partial; S findings only, no D/C or clean result                                                                        |
| `Uncharacterized`              | Unknown agent | Generic JSONL fallback                                                                                                  | No native source contract; bounded generic framing                                                                                                                                                                                                                          | No detector-grade fact contract                                                                                                                                                                                                                                                         | Uncharacterized                                                                                                         |

The Claude API-error fixture also characterizes quota limit families and reset
clocks in `isApiErrorMessage` text. Session-limit and weekly-limit messages
retain their family, hour, minute, and named time zone without retaining the
message text. The desktop resolves the clock against the incident timestamp;
a missing or unusable reset does not erase the observed refusal. This shape
adds no clean-result eligibility. See the quota incident contract in
[check coverage](check-coverage.md).

## Provider Routes

Provider identity, API shape, and model identity are separate facts. A model
name alone does not establish option or accounting semantics.

Lifecycle execution metadata reads the provider and model from the same newest
published modeled turn. It keeps harness identity, recorded provider, canonical
route, and model-family vendor separate. Pi's `openai-codex` route normalizes to
`openai`; intermediary routes never become the model vendor. Missing or custom
routes do not use a harness or model fallback for HUD provider sweeps. This is a
consumer of existing durable evidence, not new parser or check coverage. See
[`session-lifecycle-events.md`](session-lifecycle-events.md#scoped-sweep-evidence).

| Agent and format          | Provider evidence                                                                      | API evidence                                                                                                                      | Model evidence                                                   | Current policy state                                                                                                           |
| ------------------------- | -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| Claude Code JSONL         | Explicit provider/API retained when present; absent pair uses the reviewed fixed route | `anthropic` / `messages`                                                                                                          | Request model and parent-call/actual child models                | Core controls/accounting assessable on reviewed routes; explicit unknown or incomplete routes do not use the fallback          |
| Codex rollout             | `session_meta.model_provider`, thread settings, and explicit inherited fork state      | `openai` / `responses` reviewed route                                                                                             | `turn_context` and request model fields                          | Request changes retain their own route and controls; custom or invalid providers fail closed                                   |
| OpenCode JSONL and SQLite | Assistant `providerID` retained in durable rows                                        | Optional API retained; direct OpenAI, Anthropic, and Google provider IDs use their reviewed native API only for model remediation | Assistant `modelID`                                              | Anthropic cache-write and OpenAI uncached-input accounting on compatible history; arbitrary variants remain unsupported effort |
| Pi V3                     | Assistant provider retained per request                                                | Native assistant API retained in durable rows                                                                                     | Assistant model and branch-local model/policy changes            | Reviewed agent-selected policy and compatible-request accounting; missing routes are not copied from an earlier model          |
| Cursor formats            | No complete provider route is retained                                                 | No API contract is characterized                                                                                                  | Some records and metadata retain model names                     | Model aliases and complete request coverage remain partial or unknown                                                          |
| Antigravity formats       | No complete provider route is retained                                                 | Installed 2.11.0 descriptor subset is researched; no complete persisted route contract                                            | Direct model strings exist; private numeric enums are incomplete | D/O findings only where facts exist; reviewed native C is unsupported                                                          |
| Other formats             | No reviewed route contract reaches evidence                                            | Unknown                                                                                                                           | Partial names can appear in generic data                         | Fail closed                                                                                                                    |

Pi T uses `EffortSemantics::AgentSelectedPolicy`. Its saved level is not the
provider's final effort after model maps or overrides. Reviewed provider/API
pairs in `model_catalog.rs` are `openai` with `responses`, `openai-responses`,
or `openai-completions`; `openai-codex` with `openai-codex-responses`;
`anthropic` with `messages` or `anthropic-messages`; and `google` with
`generate-content` or `google-generative-ai`. Each still needs a reviewed model.
Google cache policy is not reviewed. Native API recognition does not authorize
custom providers or prove provider-translated effort.

OpenCode and Pi C use persisted provider/API fields in `TurnRow` and the shared
compatible-request query. Unknown routes, mixed accounting, missing linkage,
compactions, or incomplete history prevent clean. OpenCode uses validated
ordered history, not `parentID` as a fabricated predecessor link.

The route columns use engine turn migration 7 and desktop migration 39. Current
parser/analyzer/evidence/coverage/resume revisions are 39/24/20/6/11. Existing
revision gates invalidate old projections and snapshots; JSON and binary
evidence round trips and full/resumed replay are covered by tests.

Discovery carries the selected `SourceFormat` and surface identity with each
source descriptor. Readers use that metadata rather than reclassifying a raw
path after discovery. SQLite readers fingerprint rows visible through the live
connection, including uncheckpointed WAL rows, and compare again after a
transaction snapshot completes.

The evidence accumulator retains at most 16,384 distinct thread UUIDs. Each UUID
must be at most 256 bytes. A new UUID after the set is full, or an oversized
UUID, makes attribution incomplete and records `Partial(CapExceeded)`. Resume
deserialization rejects an oversized set or UUID. Defensive reconstruction also
caps invalid in-memory resume state and keeps the evidence partial.
The retained evidence memory ceiling is 8 MiB per accumulator. A synthetic
16,384-record linked chain with maximum-length identities verifies complete
coverage and resume round trips within that ceiling.

## Companion Sources

| Agent       | Companion                                                                   | Current use                                                                                        | Required contract                                                                                                                                                                                                                             |
| ----------- | --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code | `subagents/agent-*.meta.json` and child transcripts                         | Exact Task/Agent call ID to unique `toolUseId` pairing; actual child models                        | Scan cursors and analysis/publication fingerprints include sidecar presence and bounded content. Changed claims are rejoined on resume; source changes reject publication. Missing, invalid, duplicate, or nested-parent claims stay partial. |
| Codex       | Child rollout files                                                         | Discovery relates owned child rollouts                                                             | Preserve parent, child, model, effort, and speed inheritance.                                                                                                                                                                                 |
| Codex       | `state_5.sqlite` thread data                                                | Not part of the current rollout reader contract                                                    | Use only thread-scoped historical rows with a stable schema and snapshot contract.                                                                                                                                                            |
| OpenCode    | SQLite `session`, `message`, and `part` rows                                | Read together in one snapshot; native task metadata must agree with child ancestry and model       | CoreV2 `session_message` is a different schema and is not read by the existing SQLite path.                                                                                                                                                   |
| Pi          | Fork source named by the version 3 header                                   | Removes inherited usage while retaining explicit policy state; branch links select their own state | Fork ancestry is not delegation. Unresolved ownership remains partial.                                                                                                                                                                        |
| Pi          | Official example-extension nested `toolResult` messages in the session      | Passive parsing of actual worker models and native call identity                                   | Finding-only; no installation or execution of the extension, no clean for arbitrary extension output.                                                                                                                                         |
| Cursor      | Chat metadata, workspace metadata, and paired `state.vscdb` databases       | Used during synthesis                                                                              | Fingerprint every contributing source and preserve structured data instead of display-only text.                                                                                                                                              |
| Antigravity | Sibling brain transcript                                                    | Paired and fingerprinted with native SQLite                                                        | Preserve its tool facts and define database/transcript ownership rules.                                                                                                                                                                       |
| Antigravity | History metadata and spawn-edge data                                        | History enriches discovery; spawn edges do not reach evidence                                      | Prove passive provenance, fingerprinting, delegation meaning, and both models before check use.                                                                                                                                               |
| Cline       | Messages-contract-v1 database, root manifest, and canonical child artifacts | Loaded as one validated source for v1; legacy metadata remains separate and fail-closed            | Pair every consumed artifact and the session-scoped SQLite rows before publishing claims.                                                                                                                                                     |

Current configuration can support a current-state assessment of a model,
compaction setting, or resource inventory. It cannot prove what a historical
request exposed unless the session records the inputs needed to select that
catalog entry. Keep current-state and historical claims separate.

The desktop also has a bounded read-only advisory resource inventory for native
Claude Code, Codex, Cursor, Copilot, Cline, OpenCode, Kiro, Amp, Antigravity,
and Devin/Windsurf contexts. It returns logical MCP server and skill candidates
with current enabled state, global or project scope, provenance, and explicit
limits. It returns no path, selector, writable target, or historical exposure.
A displayed unused resource also needs a supporting failed session. Skill
candidates can carry a proportional estimate for listing frontmatter only. The
body is not read for the estimate. This inventory is not session evidence.

The reviewed current inputs are standard Claude user and project MCP files,
`.claude/skills`, skill overrides, and exact permission controls; trusted Codex
user and project `mcp_servers` layers plus `.agents/skills` and compatibility
`.codex/skills`; OpenCode JSON/JSONC `mcp` and `mcp.servers` shapes, standard
skill roots, `tools`, and permission controls; and Pi `defaultTools`, standard
skill roots, explicit non-pattern skill directories, and
`pi-mcp-extension` 1.5.0. Copilot uses `~/.copilot/mcp-config.json`, project
`.mcp.json`, and `.github/mcp.json` with its top-level `servers` map. Cline uses
its reviewed MCP settings and canonical skill roots. These are current-state
inputs, not historical session evidence. Pi MCP activation additionally requires an installed
manifest with the exact package name, version, and `./src/index.ts` extension.
Its reviewed producer is commit
`8a01fc53f3289d2e8eb492d67ba45cd84d64e7f2`.

Known candidates survive malformed or dynamic unrelated inputs. The inventory
records those inputs as limits. Indexed `SessionEvidence` can add observed MCP,
skill, and catalog-backed built-in candidates, including candidates from a
partial observed subset. It never turns that subset into current enablement or
historical completeness.

The native 30-day report reduction now builds a separate bounded resource
assessment. It reads positive use from tool counts, invoked loaded sources,
catalog-backed tool definitions, and persisted initial-context source rows.
Positive facts from partial evidence can suppress an exact candidate. Partial
evidence cannot prove non-use or a clean result. The reducer retains at most
4,096 distinct positive-use identities, 4,096 resource turn groups, 4,096
measured resource-session entries, 512 candidates and unused targets per
detector, three supporting sessions per target, 256 repository roots, and 256
distinct agent, working-directory, and repository inventory contexts. A cap
blocks clean but does not remove retained findings.

Global use applies only to the same agent. Project use applies only to the
canonical accessible repository root selected by longest path containment.
Unknown source origin does not create a scoped fallback target. When a raw call
has no source origin, a same-name project candidate in that repository takes
precedence over the global candidate. Same-name resources in different scopes
or repositories remain separate.

The pinned positive identities are case-insensitive exact resource names;
`mcp__<server>__<tool>` for Claude Code and Codex; `<server>_<tool>` for
OpenCode; and the default `mcp_<sanitized-server>_<tool>` shape from
`pi-mcp-extension` 1.5.0. Claude Code and Codex additionally accept one unique
bare suffix for a namespaced skill and the final segment of a catalog-backed
built-in alias. Ambiguous skill aliases suppress no target and block clean. An
ambiguous OpenCode or Pi MCP call suppresses all possible matching server
findings and blocks clean without counting a server as used. OpenCode and Pi use
exact skill and built-in identities.

The Checks payload and target action command use this assessment for M/B/K
status, counts, named targets, and estimates. Skill listings replicate only the
frontmatter estimate across applicable turns. MCP estimates require measured
indexed definition tokens. Built-in estimates use measured Claude Code and
Codex definitions or pinned OpenCode and Pi catalog captures. Missing inputs,
caps, and arithmetic failures omit the estimate instead of fabricating one.

Resource Auto Fix requires both indexed provenance and one exact current editor
resolution for the same agent, name, kind, scope, value, and physical key.
Inventory-only and unresolved targets remain prompt-only. Because M/B/K absence
cannot verify a transition, these prompt-only actions create no durable watch.
A successful resource Auto Fix keeps a crash-recovery record whose verification
and savings states are unavailable. It does not claim that later evidence will
verify the change.

For native sessions, the desktop can add nullable model and reasoning remediation
metadata when it publishes `Ready` evidence. This applies to `ClaudeJsonl`,
`CodexRolloutJsonl`, `OpenCodeJsonl`, `OpenCodeSqliteV2`, and `PiV3Jsonl` as the
vendor matrix allows. It hashes the physical setting and saves the effective
scope and value only when complete model evidence matches the effective setting.
Claude Code and Codex require their reviewed fixed routes. OpenCode and Pi
require the saved provider and model route. Reasoning also requires the exact
saved level. Codex project attribution requires explicit trust and resolves
reviewed `.codex/config.toml` layers from the repository root through the
session CWD. Untrusted workspaces and unsupported precedence store no
attribution. Native Windows can store attribution but cannot apply a change.
WSL stores no native attribution. This metadata describes publication-time
configuration. It is not session evidence or historical truth.

Model and reasoning Auto Fix remains pinned to this publication-time scope and
physical target. Current resolution must match both before prepare and apply.
An inherited setting therefore edits its global or user winner, while an exact
explicit project setting edits that project winner. Scalar controls are not
batched across layers. Findings from different projects group when they resolve
to the same global target, and every grouped project context is revalidated.
The editor never creates a project config.

Fast-mode remediation does not use publication-time configuration attribution.
It needs explicit persisted fast-tier session evidence and an existing current
winning Claude `fastMode = true` or Codex `service_tier = "fast"` target. Model
names, variants, labels, and latency do not qualify. Claude writes
`fastMode = false` to the one winning control rather than removing the key.

The resolver rejects runtime, environment, managed, remote, dynamic,
split-route, malformed, and ambiguous winners. Cursor and Antigravity
configuration remains separate from their accepted session contracts, so it
cannot attribute a historical setting. See the
[config attribution contracts](check-coverage.md#config-attribution-contracts).

K remediation uses the finding's exact skill identity and complete invocation
coverage. Current accepted sources do not retain a durable skill path. The
editor therefore derives a path only when one current standard `SKILL.md`
definition wins under the vendor's documented configuration roots. A missing or
ambiguous definition makes Auto Fix unavailable. The edit changes only the
vendor control and never deletes or changes `SKILL.md`.

Claude B remediation uses one exact optional web-search tool name. A project
target requires the bare canonical name in that project's exact
`permissions.allow` array. Otherwise an inherited tool resolves to global
settings. This is the only approved remediation path that can create a missing
global config, and it never creates a project config. This editor behavior does
not expand the accepted `ClaudeJsonl` history or B finding eligibility.

The winning evidence-publication transaction now enrolls at most 100 exact
passive T, O, and F findings. Enrollment starts only after desktop schema V45 is installed;
the V45 migration does not scan or infer attempts from older evidence. The
publication time in milliseconds is the immutable verification boundary, so a
historical session first published after rollout cannot become a retroactive
win. A losing claim publishes no attempt. A replay reuses the active durable
target and does not move its boundary. This work reads the bounded published
evidence and normalized turn rows. It does not read prompts, target-list state,
or window state, and it does not add a source scanner.

When no trusted workspace identifies a target, the remediation fallback scope
hash includes both the agent and session ID. Equal session IDs from different
agents cannot share a fallback target identity.

The evidence worker alternates ready evidence and remediation work when both
queues have work. Each publication and verification pass keeps its existing
bound. A winning publication can inspect its normalized user-content rows for a
bounded exact remediation marker. This activates only the matching copied-prompt
attempt at the publication boundary. It does not retain markers in evidence,
analytics, diagnostics, or derived finding facts. Restart recovery uses the
database rows; it does not rescan retained or deleted transcripts to reconstruct
attempts or contributions.

## Known Contract Gaps

- OpenCode WSL discovery launches the OpenCode executable for bounded metadata
  queries and session export. This is supported discovery, but it is not
  disk-only passive file access and does not establish a normal persisted-store
  contract for a new source format.
- Cursor store and IDE synthesis drops structured calls, arguments, usage,
  settings, and relation fields that can exist in native records.
- Antigravity private protobuf parsing does not yet preserve stable request
  identity, numeric model enums, retry meaning, provider boundaries, or
  compaction semantics.
- No current M/B/K reader proves a full historical inventory. Observed subset
  completeness plus complete calls can support scoped findings, not clean. B
  Auto Fix uses the Claude Code source only when the finding provides one exact
  canonical tool name. The advisory inventory has no mutation path. The report
  and action integration can select an Auto Fix target only when indexed
  provenance and the existing exact mutation resolver identify the same current
  resource, scope, value, and physical key. Other targets remain prompt-only.
- Selected OpenCode skills and Codex skill documents are observed injection and
  invocation, not unused listing overhead. Claude M/K also remain subset-scoped.
- OpenCode CoreV2 `session_message` is not the current `OpenCodeSqliteV2` table
  contract. Pinned schema research does not add a production reader.
- OpenCode variants have no historical effort map. Pi policy is characterized
  as agent-selected, not translated provider effort.
- Cursor permits direct O findings; Antigravity permits D/O findings where
  facts exist. Both deny clean by source gate. Neither borrows a previous record's model/time;
  Cursor retains its explicit synthesized header-model behavior.
- Generic and fail-closed readers must not promote recognized-looking fields to
  detector-grade evidence.

## Pinned First-Tier Sources

- Claude Code main JSONL fields and the child sidecar are private. The accepted
  record subset is pinned to the public 2.1.220-2.1.246 observation contract in
  [cclens][claude-session-source]. The main transcript and `.meta.json` are
  separate contracts. A missing sidecar, missing worker model, ambiguous join,
  or unknown evidence-bearing record blocks clean results.
- Codex rollout rows are pinned to the public recorder at
  [`e7637306`][codex-recorder-source]. The source writes `session_meta`,
  `turn_context`, `event_msg`, `response_item`, ordinals, and `compacted` rows.
  Synthetic fixtures cover accepted rows and loss paths. Persisted config is
  not treated as historical session evidence.
- Pi V3 is pinned to the public session-format document at
  [`b2602be7`][pi-session-source]. It defines `responseModel`,
  `providerThinkingLevel`, diagnostics, cache buckets, `id`/`parentId`, and
  the separate legacy `firstKeptEntryId` and newer retained-tail compaction
  forms. `providerThinkingLevel` is not agent-selected effort.
- Cursor's legacy CLI store and chat store use different contracts. The reviewed
  chat source uses `~/.cursor/chats/<workspace>/<session>/store.db`; both use
  only the reviewed `blobs` and `meta` subset.
  Neither source establishes an effective model fallback, inventory, route, or
  IDE configuration contract.
- Antigravity CLI SQLite is pinned to agy 1.0.16 reverse-engineering and the
  descriptor-backed field subset in ccusage. The reader admits only
  `user_version = 1`; it does not claim protobuf fields outside the tested
  usage, model, timestamp, retry, and identity subset.

[claude-session-source]: https://github.com/lambdalisue/cclens/blob/8246ffa3/docs/specs/session-format.md
[codex-recorder-source]: https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/rollout/src/recorder.rs
[pi-session-source]: https://github.com/badlogic/pi-mono/blob/b2602be77cb7b0de45dd616407fd210daa48aa75/packages/coding-agent/docs/session-format.md

## Update Rules

Update this document in the same change when any of these items changes:

- Agent discovery paths or source precedence.
- A `SourceFormat` variant or its classification rules.
- Framing, size limits, snapshot, streaming, or resume behavior.
- Parsed metrics, content, evidence, or unknown-record behavior.
- Companion discovery, pairing, fingerprinting, or ownership.
- Provider, API, model, option, or accounting extraction.
- Characterization fixtures or supported version ranges.

Update [`check-coverage.md`](check-coverage.md) in the same change when the
parsing change affects a check's finding, clean, partial, or unavailable state.

## Coverage Promotion Rule

A source is `Characterized` when it can support its documented scope. A source
is complete for clean results only when:

- Its source path and accepted shape are explicit through a schema, header, or
  pinned producer commit plus synthetic fixtures; release ranges are recorded
  where known.
- Discovery reaches the production reader with the expected `SourceFormat`.
- Framing and snapshot behavior are bounded and fail closed.
- Every contributing companion is paired and fingerprinted.
- Positive, negative, incomplete, malformed, and unknown-shape fixtures exist.
- Full and resumed reads are equivalent where resume is supported.
- Provider and model semantics are reviewed where parsing exposes controls or accounting.
- The corresponding rows in `check-coverage.md` match tested behavior.

Use `Partial` for a safe scoped result. Use `Uncharacterized` when the reader
does not support a claim. Agent characterization, resume, replay, and desktop
companion tests check behavior separately. The
[confirmation ledger](check-coverage.md#confirmation-ledger) records reviewed
source limits.
