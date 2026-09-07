# Session Parsing Coverage

Audit date: 2026-09-07.

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

Unknown versions and shapes must not fall through to a generic interpretation
that can produce a clean result. Full and resumed reads must produce equivalent
normalized evidence where resume is supported.

## Target Agents

Full passive-analysis support is required for these agents:

| Agent | Current source families | Current parsing state | Main remaining work |
| --- | --- | --- | --- |
| Claude Code | Session JSONL and discovered child sidecars | Characterized core, partial evidence lifecycle | Complete route, resource exposure, delegation, speed, and cache boundaries. |
| Codex | Rollout JSONL and discovered child rollouts | Characterized core, partial inherited state | Complete provider, resource, built-in, speed, and accounting contracts. |
| OpenCode | Legacy export JSONL and SQLite V2 | Characterized core, partial semantics | Parse native delegation, resource metadata, variants, versions, and accounting routes. |
| Pi | Version 3 session JSONL | Characterized core, partial semantics | Resolve model controls and research missing delegation, resource, speed, and cache facts. |
| Cursor | CLI agent JSONL, CLI store DB, IDE composer, and legacy chat | Partial or uncharacterized by surface | Preserve native structured data and characterize each surface separately. |
| Antigravity | Brain JSONL, cascade JSON, workspace chat, and native SQLite | Partial or uncharacterized by surface | Pin private formats and preserve identity, relations, resources, models, and accounting. |

Do not leave a check unavailable for a target agent until all relevant passive
native sources have been inspected. Ask the maintainer to approve the exact
agent/check pair when the required fact is not persisted.

## Source Matrix

The `SourceFormat` names in this table must match
`crates/antiburn-local/src/analysis/evidence.rs`.

| `SourceFormat` | Agent | Native source | Discovery and framing | Parsed facts | State |
| --- | --- | --- | --- | --- | --- |
| `ClaudeJsonl` | Claude Code | `~/.claude/projects/<workspace>/*.jsonl` | Native file discovery; bounded append-only JSONL; resume supported | Request usage, token classes, timestamps, models, effort, speed, tools, skills, MCP exposure, thread links, compactions, and child observations | Characterized core; partial lifecycle and route coverage |
| `CodexRolloutJsonl` | Codex | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` | Native file discovery with child rollouts; bounded append-only JSONL; resume supported | Request usage, token classes, timestamps, models, effort, service tier, tools, version, spawn records, compactions, and thread scope | Characterized core; partial inherited and resource state |
| `OpenCodeJsonl` | OpenCode | Legacy exported session JSONL | Native or WSL export discovery; bounded JSONL | Request usage, token classes, timestamps, models, provider IDs, variants, tools, compactions, session and record identities | Characterized core; partial delegation, resource, and provider semantics |
| `OpenCodeSqliteV2` | OpenCode | `~/.local/share/opencode/opencode.db` or platform equivalent | Read-only database snapshot; V2 tables are selected | Session, message, and part rows with usage, models, provider IDs, variants, tools, compactions, and identities | Characterized core; private version range and accounting remain partial |
| `PiV3Jsonl` | Pi | `~/.pi/agent/sessions/**/*.jsonl` or `PI_AGENT_DIR` | Native file discovery; version 3 header; bounded append-only JSONL; resume supported | Request usage, token classes, timestamps, provider, API, model, thinking level, tools, record links, forks, and compactions | Characterized core; option and resource semantics remain partial |
| `CursorJsonl` | Cursor | In-memory or compatibility JSONL without a source marker | Dedicated reader with bounded JSONL; native surface is unknown | Generic Cursor role, content, timestamp, model, tool call, and record ID fields | Uncharacterized compatibility format |
| `CursorCliAgentJsonl` | Cursor | `.cursor/projects/*/agent-transcripts/**` with chat metadata | Native discovery; transcript and metadata synthesis; bounded JSONL reader | Role, content, timestamps, models, tool calls, and selected record IDs | Partial |
| `CursorCliStoreDb` | Cursor | Cursor CLI `chats/**/store.db` | Read-only database extraction into marked JSONL | Scalar messages, title, workspace, timestamps, model, IDs, and fork-prefix hints | Partial; structured records are reduced during synthesis |
| `CursorIdeComposer` | Cursor | Workspace and global `state.vscdb` composer data | Paired database discovery and synthesis into marked JSONL | Composer identity, title, workspace, timestamps, model, messages, bubble IDs, and an `isSubagent` hint | Partial; structured calls and relations are reduced during synthesis |
| `CursorLegacyChatJson` | Cursor | VS Code-family `chatSessions/*.json` | Native file discovery; dedicated fail-closed profile | No detector-grade fact contract | Uncharacterized |
| `AntigravityJson` | Antigravity | Internal compatibility profile | Not emitted by current source classification | Shared partial Antigravity JSON facts | Internal profile; not a native source |
| `AntigravityBrainJsonl` | Antigravity | Brain transcript JSONL from CLI, IDE 2.0, or legacy paths | Native file discovery; bounded JSONL | Step usage where present, timestamps, direct models, tool calls, and selected tool input | Partial |
| `AntigravityCascadeJson` | Antigravity | API cascade or configured mirror JSON | Native or configured file discovery; bounded whole-document parsing | Nested steps, usage, timestamps, direct models, tool calls, and selected arguments | Partial |
| `AntigravityWorkspaceChatJson` | Antigravity | Workspace `chatSessions/*.json` | Native file discovery; dedicated fail-closed profile | No detector-grade fact contract | Uncharacterized |
| `AntigravitySqlite` | Antigravity | Native `conversations/<uuid>.db` with an optional sibling brain transcript | Read-only database snapshot; private protobuf subsets; companion fingerprinting | Ordered generation and step usage, retries, all token classes, timestamps, model strings, and tool rows from the companion | Partial; identity, model enums, route, and linkage remain incomplete |
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
| Claude Code `ClaudeJsonl` | The agent route is treated as Claude Code; alternate persisted routes need characterization | Reviewed Claude Code messages route | Request and session model fields | Fixed-route core exists; alternate route coverage is partial |
| Codex `CodexRolloutJsonl` | `session_meta.model_provider` and thread provider settings can persist, but inheritance is incomplete | Reviewed Codex responses route | `turn_context` and request model fields | Fixed-route core exists; provider inheritance is partial |
| OpenCode JSONL and SQLite | Assistant `providerID` persists | The effective provider API is not retained completely | Assistant `modelID` persists | Variant and accounting policy remain partial until provider/API/model resolution is complete |
| Pi `PiV3Jsonl` | Assistant provider persists | Assistant API persists | Assistant model and model changes persist | Route extraction exists; thinking and accounting mappings remain partial |
| Cursor formats | No complete provider route is retained | No API contract is characterized | Some records and metadata retain model names | Model aliases and complete request coverage remain partial or unknown |
| Antigravity formats | No complete provider route is retained | Private API and protobuf shapes are not pinned | Direct model strings exist; private numeric enums are incomplete | Model and accounting policy remain partial |
| Other formats | No reviewed route contract reaches evidence | Unknown | Partial names can appear in generic data | Fail closed |

## Companion Sources

| Agent | Companion | Current use | Required contract |
| --- | --- | --- | --- |
| Claude Code | `subagents/agent-*.meta.json` and child transcripts | Discovery can relate children and expose `toolUseId` metadata | Fingerprint the pair and join only characterized spawn identities. |
| Codex | Child rollout files | Discovery relates owned child rollouts | Preserve parent, child, model, effort, and speed inheritance. |
| Codex | `state_5.sqlite` thread data | Not part of the current rollout reader contract | Use only thread-scoped historical rows with a stable schema and snapshot contract. |
| OpenCode | SQLite `session`, `message`, and `part` rows | Read together in one snapshot | Add `event` or `session_message` only after schema and duplication rules are characterized. |
| Pi | Fork source named by the version 3 header | Used to remove inherited rows and preserve thread continuity | Keep fork ancestry separate from delegation. |
| Cursor | Chat metadata, workspace metadata, and paired `state.vscdb` databases | Used during synthesis | Fingerprint every contributing source and preserve structured data instead of display-only text. |
| Antigravity | Sibling brain transcript | Paired and fingerprinted with native SQLite | Preserve its tool facts and define database/transcript ownership rules. |
| Antigravity | History metadata and spawn-edge data | History enriches discovery; spawn edges do not reach evidence | Prove passive provenance, fingerprinting, delegation meaning, and both models before check use. |
| Cline | Metadata and message transcript | Not loaded as one complete analysis source | Pair and fingerprint both files before parsing claims change. |

Mutable current configuration can support a reviewed model or tool catalog. It
cannot prove what a historical request exposed unless the session records the
inputs needed to select that catalog entry.

## Known Contract Gaps

- OpenCode WSL discovery currently launches the OpenCode executable to query and
  export its database. This conflicts with the passive-source contract. Remove
  the process path or document and approve a narrow exception.
- Cursor store and IDE synthesis drops structured calls, arguments, usage,
  settings, and relation fields that can exist in native records.
- Antigravity private protobuf parsing does not yet preserve stable request
  identity, numeric model enums, retry meaning, provider boundaries, or
  compaction semantics.
- Claude Code and Codex resource lifecycle evidence needs complete observation
  boundaries before unused-resource checks can return clean.
- OpenCode and Pi model-control labels need reviewed provider/API/model mappings.
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

- Its source path, format, and supported version range are explicit.
- Discovery reaches the production reader with the expected `SourceFormat`.
- Framing and snapshot behavior are bounded and fail closed.
- Every contributing companion is paired and fingerprinted.
- Positive, negative, incomplete, malformed, and unknown-shape fixtures exist.
- Full and resumed reads are equivalent where resume is supported.
- Provider and model semantics are reviewed where parsing exposes controls or accounting.
- The corresponding rows in `check-coverage.md` match tested behavior.
