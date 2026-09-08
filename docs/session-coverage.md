# Session Parsing Coverage

Audit date: 2026-09-08.

This document records how Antiburn discovers and parses local session sources.
It covers source identity, framing, companion data, normalized facts, and
provider-route extraction. See [`check-coverage.md`](check-coverage.md) for the
nine burn checks that can use those facts.

This is a living contract. A discovered path does not prove that its contents
are understood. A parsed field does not prove complete historical coverage.

## Status Rules

| Status | Meaning |
| --- | --- |
| Characterized | Committed fixtures define the accepted source shape and important failure cases. |
| Partial | The reader parses useful facts, but source shapes, versions, companions, or completeness rules still have gaps. |
| Uncharacterized | Discovery can identify the source, but no detector-grade parsing contract exists. The reader must fail closed. |
| Not a session | The discovered data does not contain a conversation session and must not inherit session coverage. |

## Pipeline Contract

Every supported source passes through these boundaries:

1. Discovery identifies the agent, native source, surface, session identity, and companions.
2. Source validation pins the accepted file boundary or database snapshot.
3. The dedicated `SessionReader` selects an exact `SourceFormat`.
4. Bounded framing rejects oversized, malformed, truncated, or unreadable records.
5. Parsing emits normalized metrics, content, and evidence observations.
6. The evidence sink records complete, partial, or unavailable facts.
7. The check contract decides whether a finding or clean result is valid.

Unknown changed evidence-bearing shapes must not fall through to a generic
interpretation that can produce clean. A known shape need not have a universal
release range: an accepted schema, header, or pinned producer commit with
synthetic fixtures can define its contract. This does not prove all historical
versions. Full and resumed reads must agree where resume is supported.

## Review Scope

The reviewed targets are OpenCode, Pi, Codex, Claude Code, and Antigravity.
Their accepted source contracts and exact unsupported checks are recorded here
and in the [confirmation ledger](check-coverage.md#confirmation-ledger).
Cursor and other agents retain basic current support with broader work deferred.
Dedicated reader registration alone does not establish usable session analysis.

## Source Matrix

The table lists all 26 `SourceFormat` names from
`crates/antiburn-local/src/analysis/evidence.rs`, each exactly once.

| `SourceFormat` | Agent | Native source | Discovery and framing | Parsed facts | State |
| --- | --- | --- | --- | --- | --- |
| `ClaudeJsonl` | Claude Code | `~/.claude/projects/<workspace>/*.jsonl` | Native discovery; bounded JSONL with source claims; resume supported | Usage, token classes, time, models, request controls/routes, calls, observed resource injection, thread links, compactions, exact Task/Agent child pairing | Characterized accepted core; observed resources are not full inventories |
| `CodexRolloutJsonl` | Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | Native discovery with child rollouts; bounded JSONL; resume supported | Usage, time, models, provider/control inheritance, service tier, tools, harness version, spawn records, selected skill documents, exact tool-search MCP exposure, compactions | Characterized accepted core; resource subsets only |
| `OpenCodeJsonl` | OpenCode | Legacy exported session JSONL | Native or WSL export discovery; bounded JSONL with validated history wrappers/order | Usage, time, models, provider/API fields where saved, raw variants, task proof, selected skills, tools, compactions, session/message identities | Characterized accepted export; no historical effort map or resource inventory |
| `OpenCodeSqliteV2` | OpenCode | `~/.local/share/opencode/opencode.db` or platform equivalent | Read-only snapshot of `session`, `message`, `part`; validated creation-time/message-ID order | Native messages and parts, task metadata joined to child models, selected skills, usage, provider/API fields, compactions, identities | Characterized table contract; not CoreV2 `session_message` |
| `PiV3Jsonl` | Pi | `~/.pi/agent/sessions/**/*.jsonl` or `PI_AGENT_DIR` | Native discovery; version 3 header; bounded JSONL; resume supported | Usage, time, provider/API/model, agent-selected thinking policy, branch/fork state, tools, links, compactions, official example-extension nested worker results | Characterized core; extension delegation is finding-only, not arbitrary extension support |
| `CursorJsonl` | Cursor | In-memory or compatibility JSONL without a source marker | Dedicated reader with bounded JSONL; native surface is unknown | Generic Cursor role, content, timestamp, model, tool call, and record ID fields | Uncharacterized compatibility format |
| `CursorCliAgentJsonl` | Cursor | `.cursor/projects/*/agent-transcripts/**` with chat metadata | Native discovery; transcript and metadata synthesis; bounded JSONL reader | Role, content, timestamps, models, tool calls, and selected record IDs | Partial |
| `CursorCliStoreDb` | Cursor | Cursor CLI `chats/**/store.db` | Read-only database extraction into marked JSONL | Scalar messages, title, workspace, timestamps, model, IDs, and fork-prefix hints | Partial; structured records are reduced during synthesis |
| `CursorIdeComposer` | Cursor | Workspace and global `state.vscdb` composer data | Paired database discovery and synthesis into marked JSONL | Composer identity, title, workspace, timestamps, model, messages, bubble IDs, and an `isSubagent` hint | Partial; structured calls and relations are reduced during synthesis |
| `CursorLegacyChatJson` | Cursor | VS Code-family `chatSessions/*.json` | Native file discovery; dedicated fail-closed profile | No detector-grade fact contract | Uncharacterized |
| `AntigravityJson` | Antigravity | Internal compatibility profile | Not emitted by current source classification | Shared partial Antigravity JSON facts | Internal profile; not a native source |
| `AntigravityBrainJsonl` | Antigravity | Brain transcript JSONL from CLI, IDE 2.0, or legacy paths | Native file discovery; bounded JSONL | Step usage where present, timestamps, direct models, tool calls, and selected tool input | Partial |
| `AntigravityCascadeJson` | Antigravity | API cascade or configured mirror JSON | Native or configured file discovery; bounded whole-document parsing | Nested steps, usage, timestamps, direct models, tool calls, and selected arguments | Partial |
| `AntigravityWorkspaceChatJson` | Antigravity | Workspace `chatSessions/*.json` | Native file discovery; dedicated fail-closed profile | No detector-grade fact contract | Uncharacterized |
| `AntigravitySqlite` | Antigravity | Native `conversations/<uuid>.db` with an optional sibling brain transcript | Read-only snapshot; private protobuf subsets; companion fingerprinting | Generation and step usage, retries, token classes, direct timestamps/model strings, companion tool rows | Partial; missing model/time stays missing; identity, enums, routes, and linkage remain incomplete |
| `CopilotCliJsonl` | Copilot | `session-state/<id>/events.jsonl` | Native file discovery; dedicated fail-closed reader | Generic JSONL metrics only; no detector-grade contract | Uncharacterized |
| `CopilotIdeChatJson` | Copilot | VS Code-family `chatSessions/*.json` | Native file discovery; dedicated fail-closed reader | No IDE-specific fact contract | Uncharacterized |
| `ClineSessionJson` | Cline | Cline metadata JSON and message companion | Metadata discovery; companion loading is incomplete | No paired detector-grade fact contract | Uncharacterized |
| `KiroSessionJson` | Kiro | Canonical workspace-session JSON | Native file discovery; dedicated fail-closed reader | No canonical detector-grade fact contract | Uncharacterized |
| `KiroChat` | Kiro | `.chat` fallback | Native file discovery; dedicated fail-closed reader | No fallback detector-grade fact contract | Uncharacterized |
| `AmpThreadJson` | Amp | `threads/*.json` | Native file discovery; dedicated fail-closed reader | No whole-thread detector-grade fact contract | Uncharacterized |
| `AmpFileChanges` | Amp | `file-changes/**/*.{json,jsonl}` | Native fallback discovery | File changes only | Not a session |
| `WindsurfWorkspaceJson` | Windsurf | Workspace chat JSON | Native file discovery; dedicated fail-closed reader | No workspace detector-grade fact contract | Uncharacterized |
| `WindsurfMirrorJson` | Windsurf | Configured mirror JSON | Configured file discovery; dedicated fail-closed reader | No mirror detector-grade fact contract | Uncharacterized |
| `WindsurfCascadeProtobuf` | Windsurf | Cascade `.pb` data | Path recognition; no bounded protobuf session parser | No parsed session facts | Uncharacterized |
| `Uncharacterized` | Unknown agent | Generic JSONL fallback | No native source contract; bounded generic framing | No detector-grade fact contract | Uncharacterized |

## Provider Routes

Provider identity, API shape, and model identity are separate facts. A model
name alone does not establish option or accounting semantics.

| Agent and format | Provider evidence | API evidence | Model evidence | Current policy state |
| --- | --- | --- | --- | --- |
| Claude Code JSONL | Explicit provider/API retained when present; absent pair uses the reviewed fixed route | `anthropic` / `messages` | Request model and parent-call/actual child models | Core controls/accounting assessable on reviewed routes; explicit unknown or incomplete routes do not use the fallback |
| Codex rollout | `session_meta.model_provider`, thread settings, and explicit inherited fork state | `openai` / `responses` reviewed route | `turn_context` and request model fields | Request changes retain their own route and controls; custom or invalid providers fail closed |
| OpenCode JSONL and SQLite | Assistant `providerID` retained in durable rows | Optional API retained; cache query has reviewed native-provider accounting cases | Assistant `modelID` | Anthropic cache-write and OpenAI uncached-input accounting on compatible history; arbitrary variants remain unsupported effort |
| Pi V3 | Assistant provider retained per request | Native assistant API retained in durable rows | Assistant model and branch-local model/policy changes | Reviewed agent-selected policy and compatible-request accounting; missing routes are not copied from an earlier model |
| Cursor formats | No complete provider route is retained | No API contract is characterized | Some records and metadata retain model names | Model aliases and complete request coverage remain partial or unknown |
| Antigravity formats | No complete provider route is retained | Installed 2.11.0 descriptor subset is researched; no complete persisted route contract | Direct model strings exist; private numeric enums are incomplete | D/O findings only where facts exist; reviewed native C is unsupported |
| Other formats | No reviewed route contract reaches evidence | Unknown | Partial names can appear in generic data | Fail closed |

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
parser/analyzer/evidence/coverage/resume revisions are 31/21/17/4/6. Existing
revision gates invalidate old projections and snapshots; JSON and binary
evidence round trips and full/resumed replay are covered by tests.

## Companion Sources

| Agent | Companion | Current use | Required contract |
| --- | --- | --- | --- |
| Claude Code | `subagents/agent-*.meta.json` and child transcripts | Exact Task/Agent call ID to unique `toolUseId` pairing; actual child models | Scan cursors and analysis/publication fingerprints include sidecar presence and bounded content. Changed claims are rejoined on resume; source changes reject publication. Missing, invalid, duplicate, or nested-parent claims stay partial. |
| Codex | Child rollout files | Discovery relates owned child rollouts | Preserve parent, child, model, effort, and speed inheritance. |
| Codex | `state_5.sqlite` thread data | Not part of the current rollout reader contract | Use only thread-scoped historical rows with a stable schema and snapshot contract. |
| OpenCode | SQLite `session`, `message`, and `part` rows | Read together in one snapshot; native task metadata must agree with child ancestry and model | CoreV2 `session_message` is a different schema and is not read by the existing SQLite path. |
| Pi | Fork source named by the version 3 header | Removes inherited usage while retaining explicit policy state; branch links select their own state | Fork ancestry is not delegation. Unresolved ownership remains partial. |
| Pi | Official example-extension nested `toolResult` messages in the session | Passive parsing of actual worker models and native call identity | Finding-only; no installation or execution of the extension, no clean for arbitrary extension output. |
| Cursor | Chat metadata, workspace metadata, and paired `state.vscdb` databases | Used during synthesis | Fingerprint every contributing source and preserve structured data instead of display-only text. |
| Antigravity | Sibling brain transcript | Paired and fingerprinted with native SQLite | Preserve its tool facts and define database/transcript ownership rules. |
| Antigravity | History metadata and spawn-edge data | History enriches discovery; spawn edges do not reach evidence | Prove passive provenance, fingerprinting, delegation meaning, and both models before check use. |
| Cline | Metadata and message transcript | Not loaded as one complete analysis source | Pair and fingerprint both files before parsing claims change. |

Mutable current configuration can support a reviewed model or tool catalog. It
cannot prove what a historical request exposed unless the session records the
inputs needed to select that catalog entry.

## Known Contract Gaps

- OpenCode WSL discovery currently launches the OpenCode executable to query and
  export its database. This remains a passive-source contract gap; the coverage
  baseline does not treat executable export as passive file access.
- Cursor store and IDE synthesis drops structured calls, arguments, usage,
  settings, and relation fields that can exist in native records.
- Antigravity private protobuf parsing does not yet preserve stable request
  identity, numeric model enums, retry meaning, provider boundaries, or
  compaction semantics.
- No current M/B/K reader proves a full historical inventory. Observed subset
  completeness plus complete calls can support scoped findings, not clean.
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

A target source is complete only when:

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

The [check coverage contract test](../crates/antiburn-local/tests/check_coverage_contract.rs)
checks all source keys and the manual matrix structure. Agent characterization,
resume, replay, and desktop companion tests check behavior separately. The
[dated confirmation ledger](check-coverage.md#confirmation-ledger) contains the
pinned upstream research and approved source-scoped limits.
