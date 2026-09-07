# Burn Check Source Coverage

Audit date: 2026-09-07.

This document covers local passive evidence only. Coverage must not use hooks,
extensions, runtime subscriptions, or agent calls to fill evidence gaps. The
current OpenCode WSL discovery exception is recorded in `session-coverage.md`.

See [`session-coverage.md`](session-coverage.md) for discovery, framing, parsing,
companion-source, and provider-route coverage for the same source formats.

## Status Rules

| Status | Meaning |
| --- | --- |
| Assessable | The current reader can produce the evidence needed for a finding and a clean result. A damaged or incomplete session can still be partial. |
| Partial | The source has useful passive evidence, but the current reader or the source cannot prove all required facts. Do not report a clean result. |
| Unsupported | The audited passive source does not save a required fact. More parsing of the same source cannot make the check assessable. |
| Unknown | The source or its relevant field semantics are not characterized. Do not infer support from a path, field name, mode name, or generic JSON shape. |

`Partial` does not mean that the related work is implemented. The limitation
tables state whether parsing exists. `Unsupported` is a passive-source result,
not a statement about what the agent can do at run time.

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

The repository does not yet pin vendor schema versions. "Fixture-backed" means
that committed synthetic fixtures cover the current parsed shape. It does not
mean that all vendor releases use that shape.

| `SourceFormat` | Passive source format | Version statement | Current reader |
| --- | --- | --- | --- |
| `ClaudeJsonl` | Claude Code session JSONL | Unversioned; current shape is fixture-backed | Dedicated |
| `CodexRolloutJsonl` | Codex rollout JSONL, with discovered child rollouts | Unversioned; current shape is fixture-backed | Dedicated |
| `OpenCodeJsonl` | OpenCode legacy exported session data | Unversioned; current shape is fixture-backed | Dedicated |
| `OpenCodeSqliteV2` | OpenCode V2 SQLite session data | V2 is detected, but no vendor version range is pinned | Dedicated |
| `PiV3Jsonl` | Pi session JSONL | Header version 3 is fixture-backed | Dedicated |
| `CursorJsonl` | Cursor compatibility JSONL without a surface marker | Unversioned and uncharacterized | Dedicated shared Cursor reader |
| `CursorCliAgentJsonl` | Cursor agent transcript JSONL | Unversioned; current synthesis is partial | Dedicated shared Cursor reader |
| `CursorCliStoreDb` | Cursor CLI `chats/**/store.db` data | Private and unversioned; current synthesis is partial | Dedicated shared Cursor reader |
| `CursorIdeComposer` | Cursor IDE composer data from `state.vscdb` | Private and unversioned; current synthesis is partial | Dedicated shared Cursor reader |
| `CursorLegacyChatJson` | Cursor IDE `chatSessions/*.json` | Unversioned and uncharacterized | Dedicated fail-closed profile |
| `AntigravityJson` | Internal Antigravity compatibility profile | Not emitted by current source classification | Dedicated shared profile |
| `AntigravityBrainJsonl` | Antigravity brain transcript JSONL | Unversioned; current shape is partially characterized | Dedicated |
| `AntigravityCascadeJson` | Antigravity API cascade or mirror JSON | Unversioned; current shape is partially characterized | Dedicated |
| `AntigravityWorkspaceChatJson` | Antigravity workspace `chatSessions/*.json` | Unversioned and uncharacterized | Dedicated fail-closed profile |
| `AntigravitySqlite` | Native `conversations/<uuid>.db` plus an optional brain transcript | Antigravity 2.0 subset from embedded descriptors; private schema | Dedicated |
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

This matrix is the safe coverage contract. It can be stricter than current
binary runtime capability flags when implementation gaps exist. An
implementation that lets a `Partial` or `Unknown` cell return `Clean` is a
coverage bug.

| `SourceFormat` | D | T | S | M | B | K | O | F | C |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ClaudeJsonl` | Assessable | Partial | Partial | Partial | Partial | Partial | Assessable | Partial | Partial |
| `CodexRolloutJsonl` | Assessable | Partial | Assessable | Unsupported | Partial | Unsupported | Assessable | Partial | Partial |
| `OpenCodeJsonl` | Assessable | Partial | Partial | Partial | Partial | Partial | Assessable | Unsupported | Unsupported |
| `OpenCodeSqliteV2` | Assessable | Partial | Partial | Partial | Partial | Partial | Assessable | Unsupported | Partial |
| `PiV3Jsonl` | Assessable | Partial | Unsupported | Unsupported | Partial | Partial | Assessable | Unsupported | Unsupported |
| `CursorJsonl` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| `CursorCliAgentJsonl` | Unsupported | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `CursorCliStoreDb` | Unsupported | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `CursorIdeComposer` | Unsupported | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `CursorLegacyChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| `AntigravityJson` | Partial | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `AntigravityBrainJsonl` | Partial | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `AntigravityCascadeJson` | Partial | Unknown | Partial | Partial | Partial | Unknown | Partial | Unknown | Unsupported |
| `AntigravityWorkspaceChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown |
| `AntigravitySqlite` | Partial | Unknown | Partial | Unsupported | Unsupported | Unsupported | Partial | Unknown | Partial |
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

## Dedicated Reader Limits

An assessable entry needs no source upgrade. It still needs complete accepted
input, recognized records, and the reviewed model policy used by the check.

| Source | Checks | Exact passive limitation | Upgrade condition |
| --- | --- | --- | --- |
| Claude | T | Effort now keeps its request-level model association, and incomplete eligible activity cannot read clean. Provider-route semantics remain incomplete. | Add reviewed provider-route mappings for every persisted route. |
| Claude | S | Sidechain links and models are saved, but an event parent alone does not always prove worker delegation. | Accept only a characterized spawn or worker relation. Keep the actual parent and child model on that relation. |
| Claude | M | Loaded MCP data and exact `mcp__<server>__<tool>` calls exist. The source does not preserve all exposure changes. | Preserve exposure boundaries and complete call coverage when the source provides them. |
| Claude | B | Calls and a built-in catalog path exist. A generic catalog does not prove the effective enabled or deferred tool surface for every version. | Pin the harness version and model. Resolve the effective enabled surface and deferred state. Test positive, negative, and incomplete sessions. |
| Claude | K | Loaded and invoked skill data preserve full identity. Reliable origin and every injection boundary remain incomplete. | Preserve reliable origin, injection boundaries, and complete invocation coverage. |
| Claude | F | The transcript can save `fast`, and incomplete eligible activity cannot read clean. Delegated scope is not complete for every request. | Preserve each request's effective speed and delegation scope. |
| Claude | C | Token classes and order exist. Accounting is explicit for this format but does not isolate every compatible request segment. | Normalize accounting per request or compatible segment. Preserve links, provider changes, and compactions. |
| Codex | T | Reasoning effort keeps its request-level model association. Inherited settings are not complete. | Preserve explicit and inherited effort with each request. Resolve it through reviewed provider and model mappings. |
| Codex | M | Tool calls can be read. The persisted rollout does not provide a complete loaded MCP inventory through the current reader. | Connect a passive, versioned loaded inventory and exact native call attribution. If no passive inventory exists, change this entry to `Unsupported`. |
| Codex | B | Tool calls and harness-version evidence now reach the check path. The effective enabled and deferred surface is not proved. | Reconstruct the effective surface for the observed version and model. |
| Codex | K | Skill listings exist outside the current check path. A listing does not prove that a skill document entered model context. | Connect listings and skill reads. Preserve full identity, origin, injection state, and complete invocation coverage. |
| Codex | F | Service-tier settings are saved, but inherited tier and delegated scope are not complete for every eligible request. | Preserve the effective tier per request and worker. Require complete eligible activity before a clean result. |
| Codex | C | Ordered usage uses explicit uncached-input accounting. Mixed accounting shapes are not complete for all records. | Pin each API accounting shape and reject mixed or missing accounting from clean coverage. |
| OpenCode | T | A variant name is saved, but an arbitrary variant name is not a reasoning-effort value. | Resolve the effective variant through the provider connection and model catalog. Preserve the request-level model association. |
| OpenCode | S | Session parent rows are saved. A parent session or fork relation does not by itself prove delegated work. | Require a characterized delegation event and retain the actual parent and worker models. |
| OpenCode | M | Tool calls exist, but the current reader has no loaded MCP inventory or exact server attribution. | Parse a passive loaded inventory and native tool-to-server identity for the applicable legacy or V2 format. |
| OpenCode | B | Tool calls exist, but no versioned effective built-in tool surface is parsed. | Parse the enabled and deferred tool surface for the detected format and version. |
| OpenCode | K | Skill calls can appear as tools, but loaded or injected skills and reliable origins are not complete. | Parse the loaded inventory and injection boundaries. Preserve full identities and invocation coverage. |
| OpenCode | F | The audited session sources do not save an effective service tier or speed setting. Routing and variant names are not speed evidence. | A new passive source must save the effective tier per eligible request and worker. Otherwise this check remains unsupported. |
| OpenCode | C | V2 SQLite uses uncached-input accounting. Legacy JSONL has no reviewed repeated-context contract. Provider semantics can change within one session. | Resolve provider and API accounting per request or compatible segment. Keep JSONL unsupported until characterized. |
| Pi | T | Thinking-level rows exist, but labels are not resolved against each model's supported mapping. | Resolve each level through the provider connection and model. Preserve the request-level association. |
| Pi | S | Continuation and fork links are saved. They do not establish a worker delegation relation or worker model. | A new passive record must identify a delegated child and its effective model. Do not use fork ancestry as a substitute. |
| Pi | M | Tool blocks do not save a complete loaded MCP inventory or reliable server origin. | A new passive source must save model-facing MCP exposure and exact call attribution. Otherwise this check remains unsupported. |
| Pi | B | Tool calls are saved, but the effective built-in tool definitions and deferred surface are not. | Add a versioned catalog only when the session saves the inputs needed to select the effective surface. |
| Pi | K | Skill invocation detail can be present, but loaded or injected skill coverage and reliable origins are incomplete. | Preserve native invocation identity and parse a complete model-facing skill inventory with injection boundaries. |
| Pi | F | Session files do not save an effective service tier or speed setting. Thinking level is not speed evidence. | A new passive source must save the effective tier per eligible request and worker. Otherwise this check remains unsupported. |
| Pi | C | Pi V3 token classes vary by the selected API and do not identify repeated paid context. | A characterized passive source must identify repeated context per request. Otherwise this check remains unsupported. |
| Cursor CLI and IDE | D | Current Cursor synthesis does not retain request usage. Session totals cannot establish request depth. | Preserve native per-request context usage from a characterized CLI or IDE record. Test the two surfaces separately. |
| Cursor CLI and IDE | T | No characterized persisted field proves effective reasoning effort. Mode names and thinking text are not settings. | Characterize an explicit effective effort field, its model mapping, and its request coverage for each surface. |
| Cursor CLI and IDE | S | Some native records can save worker identity, but current synthesis drops structured relation data and models. | Preserve a native spawn relation, parent model, worker model, record identity, and timestamps. Test CLI and IDE separately. |
| Cursor CLI and IDE | M | Structured tool calls can be saved, but synthesis loses attribution arguments and no complete loaded inventory reaches evidence. | Preserve calls and exact server arguments. Parse complete historical model-facing MCP exposure for each surface. |
| Cursor CLI and IDE | B | Tool calls can be saved, but no effective enabled or deferred built-in surface reaches evidence. | Preserve structured calls and reconstruct a versioned effective tool surface for each surface. |
| Cursor CLI and IDE | K | The audit has not proved a complete persisted skill inventory, injection boundary, and invocation contract. | Characterize all three facts separately for CLI and IDE before enabling the check. |
| Cursor CLI and IDE | O | Model names can be saved and the reader emits some names. Current check coverage lacks complete timing and source-shape coverage. | Preserve model identity and timing for every eligible request. Pin aliases and reviewed replacement policy per surface. |
| Cursor CLI and IDE | F | The audit has not proved a persisted effective speed tier. A fast model, routing preset, or latency is not a tier. | Characterize an explicit effective tier with request and worker scope for each surface. |
| Cursor CLI and IDE | C | Current synthesized records do not retain the per-request token classes needed for repeated-context accounting. | Preserve characterized token classes, provider accounting, order, links, and compaction boundaries. |
| Antigravity file | D | Some steps save direct usage, but brain traces usually omit usage and mixed file shapes do not prove complete request coverage. | Declare capability per file shape. Require per-request usage coverage or pair the session with its native database. |
| Antigravity file | T, F | No characterized file field proves effective reasoning effort or speed tier. Thinking text, latency, and model names are not settings. | Characterize explicit effective settings with request and worker scope. |
| Antigravity file | S | Captured relationship sidecars can contain links, but the current reader does not connect them or prove delegation. | Pair and fingerprint the sidecar. Preserve a native delegation relation and both effective models. |
| Antigravity file | M, B | Tool calls can be saved, but model-facing MCP inventory, exact server origin, and effective built-in definitions are incomplete. | Parse exposure, attribution, and the versioned enabled surface independently for each file shape. |
| Antigravity file | K | The audit has not proved loaded and invoked skill evidence with reliable origins. | Characterize inventory, injection, invocation, and origin for each file shape. |
| Antigravity file | O | Direct model values can exist, but placeholders and shape-specific gaps prevent complete canonical identity coverage. | Resolve saved model enums or aliases and require identity plus timing on every eligible generation. |
| Antigravity file | C | Characterized files do not provide complete cache-write and cache-read classes per request. | Use the native database or characterize another passive source with all required token classes and request order. |
| Antigravity SQLite | T, F | Usage rows do not expose characterized effective effort or speed settings. Thinking-token counts and latency are not settings. | Add support only if a passive row saves the explicit effective setting and request scope. |
| Antigravity SQLite | D | Usage rows provide request token counts, but they do not prove complete logical thread membership for a clean result. | Preserve a characterized thread identity or pair a source that provides it. Keep missing links partial. |
| Antigravity SQLite | S | The database usage rows do not prove delegation. Separate relationship sidecars are not connected. | Pair and fingerprint a characterized sidecar that proves delegation and both models. |
| Antigravity SQLite | M, B, K | Generation and step usage rows do not contain a complete model-facing resource inventory or reliable resource origins. | Pair a characterized passive resource source with matching observation boundaries. Otherwise these checks remain unsupported. |
| Antigravity SQLite | O | Model enums and names are saved, but the private mapping and unknown values are not a complete reviewed identity policy. | Pin descriptor-derived mappings and preserve unknown models as unknown. Require complete generation timing. |
| Antigravity SQLite | C | The database saves all token classes, retries, timestamps, and model names. The private schema and request linkage still limit a clean churn cause. | Pin the accounting subset. Preserve compatible request order and links, mixed-provider boundaries, and compactions. |

## Uncharacterized Source Limits

All sources in this section now reach dedicated readers with distinct source
formats. Their detector-grade capabilities remain unavailable until fixtures
establish the persisted semantics. Therefore no entry in this section is
implemented as assessable, and missing evidence cannot produce a clean result.

| Source | Checks | Exact passive limitation | Upgrade condition |
| --- | --- | --- | --- |
| Copilot CLI | D, C | The official persisted event log omits per-call usage. Usage events are transient. | Find and characterize an equivalent passive source with per-request token classes and order. Otherwise these checks remain unsupported. |
| Copilot CLI | T | Persisted model or effort changes can exist, but no dedicated reader keeps effective request association. | Add a versioned reader for explicit effective effort, inheritance, model, and request boundaries. |
| Copilot CLI | S | Subagent configuration can be persisted, but configuration alone does not prove an actual delegated relation or model use. | Parse actual spawn and worker records with parent and worker models. |
| Copilot CLI | M | MCP attribution can be persisted, but the official loaded inventory and per-call usage events are transient. | Demonstrate an equivalent passive loaded inventory and complete exact call attribution. Otherwise change this entry to `Unsupported`. |
| Copilot CLI | B | Loaded built-in inventory and per-call usage are transient in the official schema. | Demonstrate equivalent passive definitions and calls for the effective surface. Otherwise this check remains unsupported. |
| Copilot CLI | K | Skill invocation records can persist, but loaded skill inventory is transient and no dedicated reader joins the facts. | Demonstrate passive loaded or injected inventory, then parse it with full invocation identity and boundaries. |
| Copilot CLI | O | Persisted model changes can identify use, but the generic reader discards them and reviewed timing coverage is absent. | Add a dedicated reader with complete model and timing coverage plus reviewed aliases and replacement policy. |
| Copilot CLI | F | Service-tier persistence and inheritance are not characterized. | Prove an explicit effective tier per request and worker before enabling the check. |
| Copilot IDE | O | Chat JSON can carry model identity, but the current generic reader does not establish complete model and timing coverage. | Add an IDE-specific reader and fixtures for all eligible message shapes. |
| Copilot IDE | D, T, S, M, B, K, F, C | IDE persistence is not characterized for the required facts. CLI schema facts do not apply to IDE storage. | Characterize the IDE source independently. Pin versions and add positive, negative, and incomplete fixtures for each check. |
| Cline pair | M, B | Message files can contain tool calls, but metadata-only discovery does not load the companion transcript. Loaded inventory and origins are not proved. | Read and fingerprint metadata with its message companion. Then characterize exposure, definitions, and exact call attribution. |
| Cline pair | O | Metadata or messages can contain model identity, but the current path does not provide complete paired timing coverage. | Pair both files and parse model identity and timing for every eligible request. |
| Cline pair | D, T, S, K, F, C | The paired source has not been characterized for complete request usage, effort, delegation, skills, speed, or cache accounting. | Characterize each JSON and database variant independently. Add companion-change and missing-companion tests. |
| Kiro canonical | M, B, K | Canonical sessions may save resource and tool data, but versioned exposure, origin, deferred state, and invocation semantics are not characterized. | Add a canonical-format reader with versioned resource semantics and complete boundaries. |
| Kiro canonical | O | Canonical sessions can carry model identity, but the generic reader provides no complete identity or timing contract. | Parse all eligible model records and timing. Add reviewed aliases and replacement policy. |
| Kiro canonical | D, T, S, F, C | Required usage, effort, delegation, speed, and accounting semantics are not characterized. | Characterize explicit fields and completeness rules before enabling each check. |
| Kiro chat | M, B | Chat fallback can contain calls, but loaded resource and effective tool-surface semantics are not characterized. | Add a separate fallback reader and prove exposure plus complete calls. Do not inherit canonical coverage. |
| Kiro chat | O | Chat fallback can contain model names, but complete identity and timing coverage are not characterized. | Parse and test the fallback independently with reviewed aliases. |
| Kiro chat | D, T, S, K, F, C | The fallback is not characterized for the required facts. | Characterize the `.chat` format independently. Do not reuse canonical declarations. |
| Amp thread | D | A whole thread can contain request data, but the current generic reader does not parse the thread or prove request context usage. | Add whole-thread parsing and require complete per-request context usage. |
| Amp thread | T, F | Routing modes are saved, but a mode name does not prove model effort or speed tier. | Map only explicit effective settings through reviewed model and provider semantics. |
| Amp thread | S | Amp supports subagents, but the current source path does not preserve an actual parent-worker relation and both models. | Parse native delegation records and retain parent and worker models. |
| Amp thread | B | Thread tool calls can exist, but the effective enabled and deferred built-in surface is not characterized. | Parse complete calls and a versioned effective tool surface. |
| Amp thread | O | Thread model data can exist, but routing modes and model identities are not separated by the generic reader. | Parse actual model identity and timing. Keep routing mode separate and apply reviewed policy. |
| Amp thread | M, K, C | Loaded MCP, loaded skills, exact origins, and cache-accounting semantics are not characterized. | Characterize each required fact and its completeness boundary before enabling a check. |
| Amp fallback | D, T, S, M, B, K, O, F, C | File-change records are not a conversation transcript. They do not save the complete request, model, resource, delegation, tier, or token facts required by any check. | Use the whole-thread source. Do not widen fallback coverage unless a new fallback schema saves the required facts. |
| Windsurf JSON | M, B | Conversation JSON can contain tool calls, but loaded MCP exposure, exact origins, and the effective built-in surface are not characterized. | Add a JSON reader with complete exposure, definitions, deferred state, and call attribution. |
| Windsurf JSON | O | Conversation JSON can contain model identity, but complete timing and alias semantics are not characterized. | Parse every eligible model record and timing. Add reviewed aliases and replacement policy. |
| Windsurf JSON | D, T, S, K, F, C | Required usage, effort, delegation, skill, speed, and cache semantics are not characterized. | Characterize explicit persisted fields and completeness rules before enabling each check. |
| Windsurf protobuf | D, T, S, M, B, K, O, F, C | Discovery recognizes Cascade paths, but there is no protobuf session parser or supported field contract. Path recognition proves no check evidence. | Derive and test a bounded passive protobuf subset for each fact. Pin the supported source version before changing any entry. |

## Coverage Promotion Rule

Change an entry to `Assessable` only when all of these conditions are true:

- The source format and supported version range are explicit.
- The reader emits every fact required for both a finding and a clean result.
- Missing, malformed, truncated, capped, or unknown records produce partial or unavailable evidence.
- Positive, negative, and incomplete synthetic fixtures exist.
- Full and resumed reads produce equivalent evidence where resume is supported.
- The model and provider policy is reviewed where the check needs policy.
- The implementation does not use current configuration as historical session evidence.

If a passive source cannot meet these conditions, keep the entry `Partial`,
`Unsupported`, or `Unknown`. Do not convert missing evidence into a clean result.
For Claude Code, Codex, OpenCode, Pi, Cursor, and Antigravity, inspect all
relevant passive native sources and get explicit maintainer approval before a
check remains `Unsupported` or `Unknown` in the coverage contract.
