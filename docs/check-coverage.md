# Burn Check Source Coverage

Audit date: 2026-10-09.

This document covers local passive session evidence and the desktop's read-only
current resource inventory. Session evidence supports historical claims. Current
inventory supports claims about what is enabled now. It cannot prove what a past
session exposed. Existing persisted output from the reviewed Pi example extension
is a passive input. The current OpenCode WSL discovery conflict is recorded in
`session-coverage.md`.

See [`session-coverage.md`](session-coverage.md) for discovery, framing, parsing,
companion-source, and provider-route coverage for the same source formats.

## Status Rules

Skill Opportunities has a registered descriptor, prompt, report, and shared UI
path for the four pinned native sources in
[retained-root smart-check inputs](session-coverage.md#retained-root-smart-check-inputs).
Its immutable current-inventory
snapshot retains semantic frontmatter, selected reference text, and optional
filesystem birth time. Only a valid nonempty description is sent as reference
text. Missing, null, or blank descriptions and plain Markdown use the full
Markdown as a fallback source; invalid YAML and non-string descriptions remain
unsupported. Large fallback sources use at most four structural 4 KiB chunks
with byte ranges and a partial flag. Known used skills follow the same rule.
Agent, project, environment, and enablement constrain candidates. Ambiguous
recorded-use identity and creation after the relevant work timestamp remain
advisory limits, not candidate exclusions; missing time stays explicit. This current-data
heuristic does not prove historical availability or contents.

Each selected skill/work comparison uses one choice: useful procedure,
specialist check, already covered, unrelated or adequate work, or uncertain.
Useful procedure and specialist check choices can support a finding at the
0.75 probability threshold. A validated positive can use partial selected work
or unknown recorded-use absence. It does not prove that the skill was unused or
that the work completed. Current source, inventory, use, and evaluator revisions
must match. A failed assessment can retain validated positive siblings; partial
or uncertain assessments cannot produce Clean.

Incomplete current inventory retains valid definitions with an explicit limit.
Report and evaluation matching validate selected reference text, source, byte
ranges, partial coverage, and definition revision against the current definition.
Work citations retain ordered UTF-8 byte ranges, total selected-action bytes,
partial status, and the full selected-action digest. The desktop evidence view
validates these bindings against the current check-selected projection. A
same-length change in an omitted range invalidates the saved citation.

Scope Creep uses one choice per selected work group. Findings require a finite
decision probability from 0.75 through 1.0 and distinguish recorded attempts
from proposals. The compact path requires selected earlier user task context;
a group without it remains unassessed. The larger-window path can admit empty
retained task context. Compact evidence labels user-authority records as task
instructions and keeps other supporting records separate. Neither path proves
complete approval history. Findings do not prove completed execution. Recorded
approvals still constrain the decision. Uncertain and unassessed groups count
separately from reviewed clean outcomes.

Scope Creep and Skill Opportunities retain large selected work, including edit
inputs, commands, and tool results, as bounded sampled content. Each large value
uses at most four structural fragments: early, task-relevant, middle, and late.
The relevant slot uses the first third when no interior child matches task terms.
Scope Creep also retains large supporting activity this way. Provider content
carries exact UTF-8 start/end byte offsets, total bytes, and partial flags. Offsets
refer to check-selected action text or the named normalized field, not native
transcript JSON. Sampled entries do not claim a complete text body. A retained
exact Bash first line does not prove complete command options.

Sampled negative decisions are Uncertain, not Clean or complete no-opportunity
results. Accepted semantic answers still complete their selected sampling jobs;
review completion does not mean all source bytes were reviewed. Validated positive
decisions can support bounded findings and recommendations with the content and
selected-window limits. No exhaustive subrange review is implemented. Compact
requests can select fewer and shorter passages than this large-value projection.
These limits do not change parser or native source-format support.

The native skill-use adapter is fixture-characterized for `ClaudeJsonl`,
`OpenCodeSqliteV2`, `CodexRolloutJsonl`, and `PiV3Jsonl` under the
[accepted shapes and limits](../crates/antiburn-local/tests/fixtures/skill_use_characterization/README.md).
Exact references preserve selected event identity, lifecycle, source roles,
timestamps/order when present, and session/publication fences. Adapters persist
generic validated skill facts before check preparation; supplemental records
cannot repair invalid selected proof. Claude 2.1.278 characterizes a failed Skill
result, not successful delivery. Older conditional launch decoding remains
separate. OpenCode load
results retain bounded parsed proof only with explicit output selection. Its typed load
result evidence is newly verified only for the pinned SQLite v2 producer
`anomalyco/opencode@772392050500e0ddcd2ad2193411a22a3824372f`, and only for a
selected, complete, untruncated, uninterrupted, uncompacted native result bound
to its session, call, message, and part. The descriptor/report path reaches this
source, but it does not establish session-wide absence or historical visibility.
OpenCode JSONL is unavailable. Other accepted agents use separate native
selection/request/result contracts. A semantic evaluation pass does not establish
product support. Codex full documents are
selection evidence, not task success. Pi explicit requests do not imply
successful extension execution. Aliases and current-file identity stay inferred.
Aggregates cannot prove absence. Even complete selected records cannot prove
session-wide or equivalent-use absence.

Optional `UserAnswer` and `PlanReference` fields have shared engine types and
private storage/query contracts. They require explicit selection. Both are
conditional for `OpenCodeSqliteV2`, `ClaudeJsonl`, `CodexRolloutJsonl`, and
`PiV3Jsonl` under the pinned contracts in `session-coverage.md`; other sources,
including Cursor and Antigravity, remain unavailable for these typed forms. OpenCode plan content
remains unresolved. Claude retains exact recorded plan text/content digests but
does not infer approved versions. Claude structured string answers retain
submitted status only for intact unambiguous results, with unknown human origin.
SDK `updatedInput`, permission changes, auto-approved tools, and `ExitPlanMode`
completion do not establish scope approval. Rejection can mean interruption;
later ordinary user free text stays separate from question results. Missing,
partial, synthetic, conflicting, or mutable evidence cannot supply authority.
Codex answer origin remains unknown; retained acceptance order is not human
authorization. Its plan records remain proposals without approved-version joins.
Pi question origin and producer identity remain unknown. Its exact plan-mode
execution choice has synthetic origin. Typed headers, context, comments, and
skipped batch status preserve meaning without widening approval eligibility.
Ordinary tool names/results cannot supply
authoritative answers. The three newer checks select these records where
supported. Missing origin and approved-version linkage remain unavailable.

| Status      | Meaning                                                                                                                                           |
| ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| Assessable  | The current reader can produce the evidence needed for a finding and a clean result. A damaged or incomplete session can still be partial.        |
| Partial     | The source has useful evidence, but its supported claims are limited. It cannot support a clean result.                                           |
| Unsupported | The reviewed passive sources do not prove a required fact or its policy semantics. This is source-scoped, not a claim about future formats.       |
| Unknown     | The source or its relevant field semantics are not characterized. Do not infer support from a path, field name, mode name, or generic JSON shape. |

For metric checks, `Partial` cannot support clean. For persisted Smart Check
columns, it marks conditional support under the pinned retained-root contract;
partial retained inputs can reach model assessment with explicit limitations.
Each check's reducer and publication policy determine scoped no-finding eligibility.
Neither status
promises exhaustive historical coverage or an implemented finding without its
source note. An unimplemented or uncharacterized
path is `Unknown` or `Unsupported`.
`Assessable` describes an accepted source contract, not every session or past
release. Metric clean results still need complete session facts and eligible activity.
Completing a Smart Check assessment does not establish complete historical context.

These statuses describe evidence capability. Runtime enablement is a separate
app-wide product gate for every check. A disabled check does not evaluate new
check-specific work or contribute active findings, clean results, counts,
remediation actions, verification watches, or savings. Its completed results,
compatible cache entries, remediation history, and incurred usage remain
stored. Re-enabling may use retained compatible results, but it does not turn
the disabled interval into Smart Check history; history remains an explicit
request. Existing local checks preserve their enabled behavior during migration,
and Ignored Instructions retains its saved group consent. Each newly introduced
check defaults off until the reader opts in. With no checks enabled, the desktop
must report that state rather than claiming Clean or Passed.

The report keeps assessment work separate from remediation lifecycle. A category
is checking only when a current, version-matched assessment in the report window
is queued or running. A failed assessment marked for continuation can carry a
continuation notice; it does not imply a running request or a clean result.
Review counts use check-owned comparison, target, or candidate units. They
are not comparable across checks. Uncertain outcomes and unselected work remain
separate. The total is unknown when source or selected-context limits prevent a
complete denominator, or any included session lacks a valid current publication
denominator. Available reviewed counts remain visible; a missing publication does
not add a zero denominator. When no publication supplies counts, coverage stays
absent. Skill review counts use the same current inventory and bound-use revision
validation as skill findings. Old evaluator revisions,
changed source identities, and mismatched publication/result revisions cannot
publish current review counts.

For the three newer Smart Checks, retained root or selected-content loss can
supply partial-context notices under the same four pinned native source contracts.
The model can assess intact retained evidence with explicit limitations instead
of rejecting the entire session for a context gap. This widens input admission
under the pinned source contracts. Findings and scoped no-finding outcomes depend
on the check's reducer and publication policy, including how it handles those
limitations. A partial-context notice does not itself prove clean or passing.
Missing context does not become recorded approval, successful execution, or
historical skill visibility. Source identity and publication revision checks
still reject stale or misbound evidence.

## Optional file-read evidence

`ReadFileRequest` and `ReadFileResult` require explicit Jev field selection.
`ReadFileOutput` separately selects recorded result text. These fields are
shared evidence APIs used by Over-exploring. Eligibility needs accepted retained
task context and bound read requests. Unrelated-file and file-breadth targets can
use requests without results; within-file targets require supported observed
extents. Findings require the actual `likely_excess` choice with finite probability
in 0.75–1.0. Available matched results bind both result ID and output digest;
half-present pairs are invalid. Independent positive targets remain available
with sibling or provider failures. Incomplete evidence never establishes Clean.
Provider windows use bounded representative text ranges with partial flags,
not complete large outputs. Review counts use completed target IDs. Smart checks
have no Auto Fix, verification, or savings estimate.

Over-exploring selects task context for each investigation instead of sending
the full session scope. It retains the latest user task and recorded question
or plan context through the investigation boundary. Assistant discussion is
supporting work, not shared task authority. Work windows retain at most 16
supporting events and 8 KiB of text, with exact target bindings and explicit
partial flags. Sampled task or work text cannot establish Clean.

Skill Opportunities groups each operation with its uniquely matched result when
available and selects optional task context at that operation's boundary. Both
checks mark task context partial when the selection omits an earlier user task.
A short approval such as "Yes, continue" cannot establish the antecedent task
or proposal. Over-exploring skips only targets without required task evidence;
other investigations can continue. Skill Opportunities can assess an interpretable
operation without task text and retain a bounded positive, but cannot establish
complete no-opportunity coverage from that missing context.
Skill-use snapshots bind the same enrollment projection as the selected work;
pre-enrollment uses remain context and do not enroll old work as a target. Settled
comparisons leave both the work inventory and its shared context before the
remaining plan runs; publication validation remains exact.

Requested line units are characterized for exact Claude `Read`, Pi `read`, and
OpenCode SQLite `read` calls under the schema and producer pins in
[`session-coverage.md`](session-coverage.md#private-file-read-evidence). Other
accepted read aliases retain native range arguments with unknown units.
Returned extents cover pinned OpenCode numbered output, Claude 2.1.278 matched
payload/numbered text, Pi 0.84.4 text/clipping details, and the Codex 0.160.1
numbered single-file shell slice. Session coverage defines exact joins, status,
and renderer limits. Cursor synthesized stores cannot supply typed tool-read
evidence; other native tool sources have no characterized returned extent.
Excluded source formats do not gain Jev support from these new types.

Missing ranges, unmatched calls, failed operations, clipped lines, directory
listings, image/PDF notices, previews, outlines, and searches do not prove that
the requested file interval was read. Shell-mediated reads other than the pinned
Codex slice are unavailable, including simple `cat`. Output bytes/digests identify recorded text,
not a file size or version. Current adapters do not supply recorded file versions.
Read counts or repeated/overlapping slices alone are not evidence of waste.

Parser revision 52 rebuilds private native facts from source; evidence schema
revision 22 is unchanged. Ignored Instructions keeps its path-only read
selection. Read ranges and read results cannot enter its
projection or change its selected content digest when the selected input and
publication fence are unchanged. Parser-bound input revisions and publication
fences still invalidate stale work through the normal revision contract.

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
| I    | Ignored Instructions  |

The other three Smart Burn Checks are Scope Creep, Over-exploring, and Skill
Opportunities. All four use Jev, Ollama, Cloudflare, or Custom connections and
provide prompt-only guidance. None supports Auto Fix, verification, or savings
estimates. Their required native agent scope is OpenCode, Codex, Claude Code,
and Pi under the source limits below. Cursor and Antigravity have separate
Ignored Instructions support; they are not supported by the three newer checks.

## Source Inventory

The tables list all 33 `SourceFormat` keys. Known source shape and release
version are separate facts. A version range is not always available; an accepted
schema, header, or pinned producer commit with synthetic fixtures can establish
a bounded contract. No row promises parity across all historical versions.

| `SourceFormat`                 | Passive source format                                                     | Version statement                                                                                                                                                                                                                         | Current reader                        |
| ------------------------------ | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| `ClaudeJsonl` | Claude Code session JSONL and child sidecars, including Claude Desktop Cowork transcripts from nested `.claude/projects` roots | Private producer; fixture-bounded core/sidecars and optional decoder pins. Smart retained roots/results use local CLI 2.1.278 accepted-log shape, including its informational prelude and explicit human markers; not SDK/GC or historical release support. A synthetic fixture characterizes Cowork main JSONL observed with Claude Desktop 2.2553.1 (embedded Claude Code 2.1.275); this does not widen the pinned range. Cowork `audit*.jsonl` is excluded. See session coverage. | Dedicated |
| `CodexRolloutJsonl` | Codex rollout JSONL with discovered children | Legacy recorder `e7637306bc9246a3e42e407cb94f96b7ed345e3e`; smart retained roots/completed items use 0.160.1 at `d27764b82f7118f674371e6d6e76271d9d606edb`. Qualified environment context is single-local and non-authorizing. No historical release range. | Dedicated |
| `OpenCodeJsonl`                | OpenCode legacy exported session data                                     | Accepted export wrappers and native message/part shapes; pinned research below                                                                                                                                                            | Dedicated                             |
| `OpenCodeSqliteV2` | OpenCode SQLite `session`, `message`, `part` tables | Fixture-backed read-only snapshot with exact IDs; optional time/title/part-ID columns. Smart message/skill producer `772392050500e0ddcd2ad2193411a22a3824372f`; numbered Read renderer `652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef`. Not CoreV2 `session_message` or a historical release range. | Dedicated |
| `PiV3Jsonl` | Pi session JSONL | Header V1/V2/V3 metric migrations; smart retained core V3 roots/results use 0.84.4 at `b79e4cc834970cca69daebffab7df1da7d1e52c4`. Optional extension question/plan pins are separate. Headerless/unsupported sources reject; no historical release range. | Dedicated |
| `OmpV3Jsonl`                   | Oh My Pi session JSONL behind the title slot                              | Fixed-width 256-byte `type: "title"` slot, then an exact version 3 header; synthetic fixtures pin the accepted shape, an allowlist admits only the OMP core rows, and every other record type or header version fails closed                | Shared Pi-family reader               |
| `CursorJsonl`                  | Cursor compatibility JSONL without a surface marker                       | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated shared Cursor reader        |
| `CursorCliAgentJsonl`          | Cursor agent transcript JSONL                                             | Separate partial export contract with content blocks and explicit subagent-path parent observations; no model fallback                                                                                                                    | Dedicated shared Cursor reader        |
| `CursorCliStoreDb`             | Legacy Cursor CLI `chats/**/store.db` data                                | Private `blobs`/`meta` subset pinned by public reverse engineering; partial                                                                                                                                                               | Dedicated shared Cursor reader        |
| `CursorChatStoreDb`            | Cursor chat `~/.cursor/chats/<workspace>/<session>/store.db` data         | Chat path and private `blobs`/`meta` subset pinned by public reverse engineering; explicit `subagentInfo.parentAgentId` is partial                                                                                                        | Dedicated shared Cursor reader        |
| `CursorIdeComposer`            | Cursor IDE composer data from `state.vscdb`                               | Private and unversioned; current synthesis is partial                                                                                                                                                                                     | Dedicated shared Cursor reader        |
| `CursorLegacyChatJson`         | Cursor IDE `chatSessions/*.json`                                          | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated fail-closed profile         |
| `AntigravityJson`              | Internal Antigravity compatibility profile                                | Not emitted by current source classification                                                                                                                                                                                              | Dedicated shared profile              |
| `AntigravityBrainJsonl`        | Antigravity brain transcript JSONL                                        | Unversioned; model-setting changes, thinking, and truncated-field markers are characterized partially                                                                                                                                     | Dedicated                             |
| `AntigravityCascadeJson`       | Antigravity API cascade or mirror JSON                                    | Unversioned; thinking and nested tool arguments are characterized partially                                                                                                                                                               | Dedicated                             |
| `AntigravityWorkspaceChatJson` | Antigravity workspace `chatSessions/*.json`                               | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated fail-closed profile         |
| `AntigravitySqlite`            | Native `conversations/<uuid>.db` plus an optional brain transcript        | agy 1.0.16 reverse-engineered subset; requires `user_version = 1` and reviewed `gen_metadata(idx,data)` or `steps(idx,metadata)` columns; exact bounded response identities; conflicting model joins are partial; not full schema support | Dedicated                             |
| `CopilotCliJsonl`              | `session-state/<uuid>/events.jsonl` plus sibling `session-store.db`       | Public Copilot SDK v1 envelope plus schema-v7 read-only request store; target-session request rows reconcile the persisted shutdown totals; unknown lanes remain partial                                                                  | Dedicated v1 bundle reader            |
| `CopilotIdeChatJson`           | VS Code-family `chatSessions/*.json`                                      | Unversioned; IDE and CLI contracts are separate                                                                                                                                                                                           | Dedicated fail-closed                 |
| `ClineSessionJson`             | Cline metadata and message companion                                      | Cline 2.0+ naming is known; message schemas are not pinned                                                                                                                                                                                | Dedicated fail-closed                 |
| `ClineMessagesContractV1`      | Cline `.cline/data/db/sessions.db`, root manifest, and messages artifacts | Cline messages-contract v1; required `sessions` columns include `agent_id`; root and child artifacts use exact paths, parent IDs, origins, and model matches                                                                              | Dedicated v1 bundle reader            |
| `KiroSessionJson`              | Kiro workspace-session JSON                                               | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated fail-closed                 |
| `KiroChat`                     | Kiro `.chat` fallback                                                     | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated fail-closed                 |
| `KiroCliV2Bundle`              | Kiro CLI V2 `.json` metadata and matching `.jsonl` journal                | Fixture-backed observed V1 metadata and V1 envelope contract; UUID siblings only; D/S unavailable and C unsupported                                                                                                                       | Dedicated V2 bundle reader            |
| `KiroCliV3Bundle`              | Kiro CLI V3 `session.json` and `messages.jsonl` directory                 | Separate path is known, but no pinned `session.json` producer shape; fail-closed                                                                                                                                                          | Dedicated fail-closed                 |
| `KiroChatSaveExport`           | Manual Kiro CLI `/chat save` JSON                                         | Public command is known; export schema is not published                                                                                                                                                                                   | Not scanned; unsupported              |
| `AmpThreadJson`                | Amp `threads/*.json` whole-thread record                                  | Explicit full-export envelope version 39; ordered assistant usage with `totalInputTokens`, `maxInputTokens`, model, timestamp, tools, and activated skills; findings only                                                                 | Dedicated v39 export reader           |
| `AmpFileChanges`               | Amp `file-changes/**/*.{json,jsonl}`                                      | File-change fallback, not a thread                                                                                                                                                                                                        | Dedicated fail-closed                 |
| `WindsurfWorkspaceJson`        | Windsurf workspace chat JSON                                              | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated fail-closed                 |
| `WindsurfMirrorJson`           | Configured Windsurf mirror JSON                                           | Unversioned and uncharacterized                                                                                                                                                                                                           | Dedicated fail-closed                 |
| `WindsurfCascadeProtobuf`      | Windsurf Cascade `.pb` data                                               | Private and uncharacterized                                                                                                                                                                                                               | Dedicated fail-closed when discovered |
| `DevinLocalSqlite`             | Devin Local migration-17 `sessions.db`                                    | Migration 17 and required table columns are fixture-pinned; WAL-visible read-only snapshots; ACP schema 6 is optional child companion only                                                                                                | Dedicated S-only reader               |
| `Uncharacterized`              | Unknown-agent generic fallback                                            | No source contract                                                                                                                                                                                                                        | Generic fail-closed                   |

## Coverage Matrix

This matrix records implemented eligibility and source limits. Skill Opportunities
and Over-exploring, plus Scope Creep, use persisted assessments, not synchronous metric gates.
Their conditional native Claude Code, Codex, OpenCode SQLite, and Pi support
requires accepted retained-root history and pinned native evidence. Other sources
remain unavailable. `Partial` marks source-scoped support; a complete eligible
assessment can report clean within reviewed scope, not exhaustive history.

Ignored Instructions has separate evidence coverage below. Its input contract
uses retained session content and instruction snapshots, not the metric gates
in this matrix.

| `SourceFormat` | D | T | S | M | B | K | O | F | C | Skill Opportunities | Over-exploring | Scope Creep |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `ClaudeJsonl` | Assessable | Assessable | Assessable | Partial | Partial | Partial | Assessable | Assessable | Assessable | Partial | Partial | Partial |
| `CodexRolloutJsonl` | Assessable | Assessable | Assessable | Partial | Partial | Partial | Assessable | Assessable | Assessable | Partial | Partial | Partial |
| `OpenCodeJsonl` | Assessable | Unsupported | Assessable | Unsupported | Unsupported | Partial | Assessable | Unsupported | Assessable | Unsupported | Unsupported | Unsupported |
| `OpenCodeSqliteV2` | Assessable | Unsupported | Assessable | Unsupported | Unsupported | Partial | Assessable | Unsupported | Assessable | Partial | Partial | Partial |
| `PiV3Jsonl` | Assessable | Assessable | Partial | Unsupported | Unsupported | Unsupported | Assessable | Unsupported | Assessable | Partial | Partial | Partial |
| `OmpV3Jsonl` | Partial | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `CursorJsonl` | Unsupported | Unknown | Unknown | Unknown | Unknown | Unknown | Partial | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `CursorCliAgentJsonl` | Unsupported | Unknown | Unsupported | Unsupported | Unsupported | Unknown | Partial | Unknown | Unsupported | Unsupported | Unsupported | Unsupported |
| `CursorCliStoreDb` | Unsupported | Unknown | Unsupported | Unsupported | Unsupported | Unknown | Partial | Unknown | Unsupported | Unsupported | Unsupported | Unsupported |
| `CursorChatStoreDb` | Unsupported | Unknown | Unsupported | Unsupported | Unsupported | Unknown | Partial | Unknown | Unsupported | Unsupported | Unsupported | Unsupported |
| `CursorIdeComposer` | Unsupported | Unknown | Unsupported | Unsupported | Unsupported | Unknown | Partial | Unknown | Unsupported | Unsupported | Unsupported | Unsupported |
| `CursorLegacyChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `AntigravityJson` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AntigravityBrainJsonl` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AntigravityCascadeJson` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AntigravityWorkspaceChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `AntigravitySqlite` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `CopilotCliJsonl` | Unsupported | Unsupported | Assessable | Unsupported | Unsupported | Unsupported | Assessable | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `CopilotIdeChatJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Partial | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `ClineSessionJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `ClineMessagesContractV1` | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `KiroSessionJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `KiroChat` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `KiroCliV2Bundle` | Unsupported | Unknown | Unsupported | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported | Unsupported |
| `KiroCliV3Bundle` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `KiroChatSaveExport` | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AmpThreadJson` | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `AmpFileChanges` | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `WindsurfWorkspaceJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `WindsurfMirrorJson` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `WindsurfCascadeProtobuf` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |
| `DevinLocalSqlite` | Unsupported | Unsupported | Partial | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported | Unsupported |
| `Uncharacterized` | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unknown | Unsupported | Unsupported | Unsupported |

## Smart Check Review Coverage

Small-window preparation uses capability-based compact requests and short
check-specific questions. Scope Creep, Over-exploring, and Skill Opportunities
select compact passages with 8,192 usable estimated tokens or fewer. Shared
request wrappers use their compact form with 4,096 or fewer. Fit checks shrink
passages within finite budgets before skipping a comparison. These are bounded selected comparisons,
not exhaustive semantic subrange review. Partial or compact sampled negatives
cannot establish complete Clean. Provider fit uses an estimate, not an exact
tokenizer, and does not establish small-model accuracy or live latency.

Native input loading omits malformed selected tool inputs and untrusted user
parts while retaining independent valid work with partial coverage. Exact source,
branch, publication, and selected dependency bindings remain required. Unknown
authority cannot become approval. Empty candidate inventories are stored as
terminal `no_candidates` work, distinct from Clean and unavailable evidence.
Loader failures retain specific internal categories; stale publications do not
publish replacement results. Skipped targets remain unreviewed and do not imply
active continuation. Normal candidate enumeration excludes terminal empty work
with unchanged source, publication, and evaluator identities.

Skill Opportunities bounds operation/skill enumeration to 4,096 comparisons.
Omitted comparisons remain explicit partial coverage and cannot establish Clean.
Continuation reviews retained candidates; it does not exhaustively enumerate
comparisons omitted by that cap.

Findings can retain a versioned comparison basis with the selected passages and
typed relationship. The evidence endpoint validates source bindings and provides
paired read requests/results with separate requested and observed extents. Skill
task citations belong to the comparison, not the request batch. Older findings
can expose available excerpts without a fabricated explanation. None of these
contracts proves complete history, execution success, or historical skill visibility.

All four production descriptors target an initial review of 50% of eligible
targets, then continue remaining work. The percentage measures the target
fraction, not model confidence or finding probability. Review counts require all
required answers and successful check reduction. Partial answers, dispatches,
private thinking, and unavailable evidence cannot count as reviewed targets.

One shared scheduler rotates across the four descriptors. Four turns prefer
initial review and the fifth prefers continuation, with the other lane available
when the preferred lane is empty. Per-turn target limits are 8 for Ignored
Instructions, 4 for Scope Creep, 3 for Over-exploring, and 4 for Skill
Opportunities. Each turn admits at most two provider dispatch attempts, including
retries. The Store retains finite attempt limits across turns and restarts;
continuation does not reset exhausted work.

Answer reuse requires the exact selected evidence, decision context, source
bindings, reference inputs, model/provider configuration, and check revisions.
Repacking alone does not invalidate a compatible answer. A changed scope or
skill reference invalidates affected answers. Reaching the initial fraction
does not turn incomplete, unsupported, uncertain, exhausted, or failed work into
Clean. These scheduling rules do not expand source or remediation support in
the matrices below. See [incremental review](smart-burn-checks.md#incremental-review-and-scheduling).

Scope Creep stops new descriptor enumeration at 512 KiB of serialized descriptor
data. Before dispatch, it also reserves space for a positive answer within the
existing 2 MiB checkpoint and 1 MiB publication limits. A storage-limited
inventory has typed partial coverage and an unknown total. It cannot produce
Clean or resume identical omitted work automatically. Accepted findings remain
available. A source or dependency change can start a new bounded inventory.
Exact Ignored Instructions work or active configuration changes invalidate Scope
Creep scheduling, including completed assessments with no runnable targets.

## Ignored Instructions Evidence

The first-tier product matrix is the source of truth for reachable check
support. Supported formats are `ClaudeJsonl`, `CodexRolloutJsonl`,
`OpenCodeSqliteV2`, `PiV3Jsonl`, `CursorCliAgentJsonl`, `CursorCliStoreDb`,
`CursorChatStoreDb`, `CursorIdeComposer`, `AntigravityBrainJsonl`, and
`AntigravitySqlite`. Cursor store and composer routes retain normalized user
and assistant text but may not retain native tool inputs. Antigravity SQLite
needs assessable companion transcript content. Other Cursor and Antigravity
formats remain unavailable. `OpenCodeJsonl` is also unavailable. Clean
means no finding among sampled comparisons in a completed review, not that all
session content is safe. Unsampled pairs remain a coverage gap. Observed
current-file versions govern future actions; the first observation cannot
prove earlier activation. An explicit history run can compare past actions to
current files, but this does not prove historical activation. Missing recorded
historical snapshots remain unavailable as proof.
Provider failures and incomplete source evidence are different from sampling
gaps and cannot produce Clean. Auto Fix, verification, and estimates are
unsupported. Each comparison uses one joint Choice decision. Missing required
evidence cannot establish a clean result.
The per-field source limits and selection states are listed in
[Smart Burn Checks selected-input coverage](smart-burn-checks.md#ignored-instructions-selected-input-coverage).

Selected evidence now retains OpenCode's typed native lifecycle labels and
field-filtered native string bindings. A completed label is not proof of a
passing result. Pinned Claude, Codex, and Pi native results also retain bounded
lifecycle facts. Only explicitly selected output can enter context. Uncharacterized
lifecycle state,
encoded-argument ranges, recorded platform, and native glob dialect remain
unknown. Truncated known requests supply metadata and a malformed-field limit,
not fabricated request text. Shared lexical path helpers need explicit recorded
context and do not expand finding or clean-result eligibility. See the
[recorded facts contract](smart-burn-checks.md#recorded-operation-and-path-facts).

## First-Tier Product Matrix

Support means reachable product behavior, not a parser flag or an unused engine
function. This documentation-only grouping does not gate runtime behavior.
`Finding` is `Y`, `FO` (a finding without a clean-result claim), or `N`.
`Prompt`, `Verification`, and `Estimate` are `Y` or `N`. `Auto Fix` is `Y`, `C`
(only with an exact production binding), `P` (prompt only), or `N`. Prompt `Y`
requires both an engine recommendation and a finding that the desktop can
reach. Estimate `Y` means the finding can reach its typed estimate or bounded
finding-rate fallback; it does not promise a numeric measured value.

The engine contract checks source findings, recommendation support, and estimate
methods. The desktop contract also requires a reachable watch before it accepts
verification, a reachable inventory target before it accepts an M/B/K prompt,
and production editor policy plus a typed target operation before it accepts
Auto Fix. The limit stays beside the row it qualifies.

Ignored Instructions uses stored Jev assessments, including bounded local excerpts
of each finding's compared instruction and action, and the current published
content projection. Finding, sampled Clean, and prompt support are limited to
the exact source formats listed above. The worker records observed
instruction versions with session positions. A changed version applies to
future actions, while a requested history run can compare older actions to
current rules as a possible issue. Without an authoritative snapshot this is
not proof those rules were active then. Clean covers sampled comparisons, not
historical activation or exhaustive session coverage. Unresolved sampled
comparisons remain unassessed.
Auto Fix,
verification, and estimates are unsupported. The check uses the configured Jev,
Ollama, Cloudflare, or Custom connection and sends selected instructions,
assistant text, user text, Bash command input/output, file-edit paths,
read-file paths, search queries with scope filters, and other-tool input. Bash
input can include inline scripts, heredocs, and patches recorded in the command;
dedicated edit-tool content stays excluded. A command request does not prove
execution or success. User text and Bash results supply bounded context only when
normalized human-history or exact request/result facts validate them. Unknown
origin, synthetic text, skill documents, unmatched/conflicting joins, and clipping
cannot establish permission or successful tests. Edit content, read/search output,
other results, typed question/plan fields, and thinking remain excluded.
Selected paths can leave the machine. Sampling selects
up to 8 high-priority rule/action pairs per turn by word rarity, tool
name, literal path, prohibition/tool-input risk, and recency. It spreads choices
across rules and sources and probes low-overlap pairs. These signals cannot
prove that omitted pairs are irrelevant. Risk-ranked new activity alternates with
older unchecked work across source chronology to reduce the remaining gap,
unless new work adds pairs. Compatible typed answers persist across append, completion, and
restart with source-bound identities and input/revision checks. Long non-command text is
split into overlapping byte ranges; not every range must be sent. Bash commands
stay atomic and remain unassessed when they exceed the request limit. Rule
targets include bounded enclosing-section and ancestor text with source byte
and line ranges. Large supporting text selects first, relevant, and last
structural ranges within the context budget. Request
size remains bounded, but this is not a per-session price or time cap. About
60 seconds after worker start for an ordinary assessment is a goal, not a
completion deadline or guarantee.
Requests retain the rule text, provenance, scope, candidate action, and bounded
same-branch evidence. One event window groups up to eight rule targets around a
candidate action, so the request sends shared event text once instead of
repeating it for each rule. Instruction-file paths and stable event IDs stay
local. Source byte and line ranges accompany selected instruction context.
Earlier context is sent in source order within its array.
Candidate actions include normalized assistant text (`assistant` content kind)
and the selected input envelope for each recognized tool category. Edit and read
inputs are reduced to paths; search inputs retain the query and scope filters.
Malformed known tools and tools without a name do not fall through to other-tool
input. Bash envelopes retain recorded execution context; search constraints
reject arbitrary nested objects. Recorded `patchText` in an OpenCode
`apply_patch` request supplies paths and operations when valid. Both patch
rename paths remain selected, while dedicated edit content remains excluded.
Selected actions retain no excluded normalized-field values. Native cross-agent
fixtures and the 4,096-mask projection test cover this boundary. Tool lifecycle requests do not prove
successful execution. Pinned native result contracts retain lifecycle labels;
other shapes and unresolved path semantics remain unknown. The
shared evidence module also supports output-enabled checks.
Claude result names require exact call IDs in the same resolved branch; missing
or conflicting joins remain unavailable. Native singleton-selection tests cover
all twelve fields and private thinking. Explicit empty text is observed rather
than inferred missing. Selected Bash output needs exact normalized context
bindings; lifecycle status alone does not prove successful tests.
Tool results do not create candidate work. Pages interleave rules from each instruction
source so a large global file cannot starve project rules. All six
first-tier agents include shared global and project AGENTS.md files alongside
their supported agent-specific sources. The Markdown reader uses headings and
list structure to bound rule text. Text before the first heading scopes each
headed rule but does not become a separate rule. It does not infer semantic rule types from
keyword lists. Jev answers one joint Choice question per target: `conflict`,
`no_issue`, `pending_completion`, or `uncertain`. The decision considers
applicability, conditions, exceptions, prerequisites, and evidence sufficiency
together. Publication requires at least 0.75 actual probability for `conflict`;
the reducer retains that value as `composite_probability`. Current-file or
truncated-action findings remain possible; stronger source/action evidence
permits likely findings under the same probability threshold. A sufficiently
probable `pending_completion` answer marks a rule pending. Missing prerequisite
evidence remains uncertain. The reducer leaves weak and uncertain decisions
unassessed. Truncated context does not block a direct conflict when
supplied evidence establishes it. Partial assessment cannot support a clean
result.

For a supported single-path file-read request prerequisite, Jev identifies the
literal prerequisite and triggering edit scope. Code compares selected read
requests in the same recorded branch and scope. Complete prior history with
known request paths can establish a missing earlier request. This finding is
limited to request order; it does not prove successful reading or execution.
Missing history, malformed paths, ambiguous reading requirements, alternative
prerequisites, and unsupported literal bindings remain unassessed. A matching
earlier request can satisfy request order without read output. Bounded prose
context does not replace the exact selected request facts.
Literal edit scopes support plain relative file and directory paths. Wildcards,
variables, absolute paths, and paths that need alias resolution remain unknown.
The reducer settles supported request order from exact facts after semantic
rule and path binding. It does not change the publication thresholds.

Approval-dependent rules cannot publish a finding or clean comparison without
accepted authorizing context. Selected human text needs source-bound history
proof, native ranges, and the same recorded branch. Unknown-origin answers,
synthetic text, skill selection, assistant reports, and tool permission do not
restore authority. Missing history stays unassessed.
A rule that prohibits claiming approval can still be assessed as observable
assistant communication. Unclear permission dependence remains unassessed.
Checkpoint reduction also requires permission classification. A prepared plan
without classified exact observations cannot publish a guessed order result.

The content reader orders events by their source positions and divides large
sessions into bounded pages. If an older page may contain a required earlier
step, the worker carries the newer candidate and its source identity forward.
It adds matching earlier events from each older page and assesses the candidate
again. The worker clears an earlier unassessed result only after the new page
covers that candidate. Missing pages never prove that an approval or
prerequisite did not happen.
New activity is assessed after three minutes of inactivity. Appended new actions
enter review even when compatible old judgments are reused. Settings can also
queue sessions active in the selected 7-day or 30-day window. Historical runs
freeze a cohort for Settings progress. Unchanged completed sessions are not
requeued by another click; due failures, changed activity, or a new evaluator
revision can be selected again. Startup requeues stale evidence when its verified
source identity, generation, or parser, analyzer, or evidence-schema revision is
old. Terminal source failures and unsupported evidence count as failed or
skipped instead of waiting for analysis forever. Historical runs use the same source and evidence
limits; they do not establish complete history outside the selected window.

| Agent       | Check | Finding | Prompt | Auto Fix | Verification | Estimate | Reachability limit                                                                                                                           |
| ----------- | ----- | ------- | ------ | -------- | ------------ | -------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code | I     | Y       | Y      | N        | N            | N        | `ClaudeJsonl` only; Clean covers sampled post-observation comparisons, not all activity.                                                    |
| Codex       | I     | Y       | Y      | N        | N            | N        | `CodexRolloutJsonl` only; Clean covers sampled post-observation comparisons.                                                                 |
| OpenCode    | I     | Y       | Y      | N        | N            | N        | `OpenCodeSqliteV2` only; JSONL export is not enabled; Clean covers sampled post-observation comparisons.                                    |
| Pi          | I     | Y       | Y      | N        | N            | N        | `PiV3Jsonl` only; Clean covers sampled post-observation comparisons.                                                                         |
| Cursor      | I     | Y       | Y      | N        | N            | N        | `CursorCliAgentJsonl`, `CursorCliStoreDb`, `CursorChatStoreDb`, and `CursorIdeComposer`; normalized store/composer routes may omit tool inputs. |
| Antigravity | I     | Y       | Y      | N        | N            | N        | `AntigravityBrainJsonl` and `AntigravitySqlite`; SQLite needs assessable companion transcript content.                                         |
| Claude Code | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                     |
| Claude Code | T     | Y       | Y      | Y        | Y            | Y        | Verification needs a later complete lower control on the same route and model.                                                               |
| Claude Code | S     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one exact named agent definition.                                                                                             |
| Claude Code | M     | Y       | Y      | C        | N            | Y        | Current inventory can find targets; Auto Fix needs indexed provenance and one exact MCP binding. Historical subsets cannot verify absence.   |
| Claude Code | B     | Y       | Y      | Y        | N            | Y        | Only the allowlisted optional tools are reachable. Historical subsets cannot verify absence.                                                 |
| Claude Code | K     | Y       | Y      | C        | N            | Y        | Current inventory can find targets; Auto Fix needs indexed provenance and one exact skill binding. Historical subsets cannot verify absence. |
| Claude Code | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                        |
| Claude Code | F     | Y       | Y      | Y        | Y            | Y        | The source must record the fast tier and one current winning control must still be fast.                                                     |
| Claude Code | C     | Y       | Y      | N        | N            | Y        | No durable cache-policy target exists.                                                                                                       |
| Codex       | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                     |
| Codex       | T     | Y       | Y      | Y        | Y            | Y        | Verification needs a later complete lower control on the same route and model.                                                               |
| Codex       | S     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one exact named agent definition.                                                                                             |
| Codex       | M     | Y       | Y      | C        | N            | Y        | Auto Fix needs indexed provenance and one exact trusted MCP table. Historical subsets cannot verify absence.                                 |
| Codex       | B     | Y       | Y      | N        | N            | Y        | The finding is catalog-backed; no reviewed built-in control binds the target.                                                                |
| Codex       | K     | Y       | Y      | C        | N            | Y        | Auto Fix needs indexed provenance and one exact trusted skill binding. Historical subsets cannot verify absence.                             |
| Codex       | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                        |
| Codex       | F     | Y       | Y      | Y        | Y            | Y        | The source must record the fast tier and one current winning control must still be fast.                                                     |
| Codex       | C     | Y       | Y      | N        | N            | Y        | No durable cache-policy target exists.                                                                                                       |
| OpenCode    | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                     |
| OpenCode    | T     | N       | N      | N        | N            | N        | Accepted sources do not retain a historical effort map.                                                                                      |
| OpenCode    | S     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one exact named agent definition.                                                                                             |
| OpenCode    | M     | Y       | Y      | C        | N            | Y        | Current inventory reaches the finding and prompt. Auto Fix additionally needs indexed provenance and one exact effective V2 MCP binding.     |
| OpenCode    | B     | Y       | Y      | P        | N            | Y        | Current inventory reaches only allowlisted optional tools; no production built-in editor policy is available.                                |
| OpenCode    | K     | Y       | Y      | C        | N            | Y        | Auto Fix needs indexed provenance and one exact standard skill winner. Historical subsets cannot verify absence.                             |
| OpenCode    | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                        |
| OpenCode    | F     | N       | N      | N        | N            | N        | Accepted sources do not retain an effective speed tier.                                                                                      |
| OpenCode    | C     | Y       | Y      | N        | N            | Y        | Cache episodes require consecutive validated order and distinct message IDs; Anthropic routes use the five-minute default unless the preceding cache write records one-hour TTL evidence.                  |
| Pi          | D     | Y       | Y      | Y        | N            | Y        | Auto Fix needs one supported current compaction control.                                                                                     |
| Pi          | T     | Y       | Y      | Y        | Y            | Y        | Effort evidence requires positive usage on that record; verification covers the saved agent-selected policy, not provider-translated effort.  |
| Pi          | S     | FO      | Y      | N        | N            | Y        | Only reviewed example-extension output can produce the finding.                                                                              |
| Pi          | M     | Y       | Y      | P        | N            | Y        | Current inventory reaches the finding and prompt, but no exact production editor binding is supported.                                       |
| Pi          | B     | N       | N      | N        | N            | N        | Pi's built-ins are core tools and never become eligible B targets.                                                                           |
| Pi          | K     | Y       | Y      | P        | N            | Y        | Current inventory reaches the finding and prompt, but no exact production editor binding is supported.                                       |
| Pi          | O     | Y       | Y      | Y        | Y            | Y        | Auto Fix and verification need publication-time physical attribution.                                                                        |
| Pi          | F     | N       | N      | N        | N            | N        | Accepted sources do not retain an effective speed tier.                                                                                      |
| Pi          | C     | Y       | Y      | N        | N            | Y        | Reviewed native API routes support repeated-input accounting and recovered miss episodes; unsupported route or episode evidence cannot produce a clean or actionable result. |
| Cursor      | D     | N       | N      | N        | N            | N        | Characterized Cursor surfaces do not emit request-depth evidence.                                                                            |
| Cursor      | T     | N       | N      | N        | N            | N        | No complete effective effort contract exists.                                                                                                |
| Cursor      | S     | N       | N      | N        | N            | N        | Current relation hints do not establish a detector-grade worker finding.                                                                     |
| Cursor      | M     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Cursor remediation recommendation exists.                                                         |
| Cursor      | B     | N       | N      | N        | N            | N        | No optional Cursor built-in is eligible for a B target.                                                                                      |
| Cursor      | K     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Cursor remediation recommendation exists.                                                         |
| Cursor      | O     | FO      | Y      | N        | N            | Y        | Characterized direct timed-model observations reach findings and prompts, but never clean or physical attribution.                           |
| Cursor      | F     | N       | N      | N        | N            | N        | No complete effective speed contract exists.                                                                                                 |
| Cursor      | C     | N       | N      | N        | N            | N        | No compatible request-accounting contract exists.                                                                                            |
| Antigravity | D     | FO      | Y      | N        | N            | Y        | Direct depth evidence is positive-only.                                                                                                       |
| Antigravity | T     | N       | N      | N        | N            | N        | No complete effective effort contract exists.                                                                                                |
| Antigravity | S     | N       | N      | N        | N            | N        | No persisted detector-grade delegation contract exists.                                                                                      |
| Antigravity | M     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Antigravity remediation recommendation exists.                                                    |
| Antigravity | B     | N       | N      | N        | N            | N        | No optional Antigravity built-in is eligible for a B target.                                                                                 |
| Antigravity | K     | FO      | N      | N        | N            | Y        | Current inventory can produce a target; no Antigravity remediation recommendation exists.                                                    |
| Antigravity | O     | FO      | Y      | P        | N            | Y        | Direct model evidence reaches a prompt, but no physical model control is attributed.                                                         |
| Antigravity | F     | N       | N      | N        | N            | N        | No complete effective speed contract exists.                                                                                                 |
| Antigravity | C     | N       | N      | N        | N            | N        | No compatible request-accounting contract exists.                                                                                            |
| Claude Code | SkillOpportunities | Y | Y | N | N | N | Native CLI 2.1.278 retained root; exact requests/failures and separate conditional launch contract; no proof of execution or historical visibility. |
| Codex | SkillOpportunities | Y | Y | N | N | N | Native 0.160.1 pinned retained root; full-document selection is not human approval or successful execution; current inventory is not historical visibility. |
| OpenCode | SkillOpportunities | Y | Y | N | N | N | `OpenCodeSqliteV2` only; uses pinned native skill-use metadata and current inventory. No historical availability, auto-fix, verification, or estimate. |
| Pi | SkillOpportunities | Y | Y | N | N | N | Native 0.84.4 core V3 retained root; wrapper selection/requests do not prove extension identity or successful execution; current inventory is not historical visibility. |
| Cursor | SkillOpportunities | N | N | N | N | N | No accepted source format or product descriptor/report path is available. |
| Antigravity | SkillOpportunities | N | N | N | N | N | No accepted source format or product descriptor/report path is available. |
| Claude Code | OverExploring | Y | Y | N | N | N | Native CLI 2.1.278 retained task context and bound Read requests; within-file targets require matched text results with observed extents; no whole-file inference. |
| Codex | OverExploring | Y | Y | N | N | N | Native 0.160.1 retained task context and pinned numbered single-file shell slice requests; observed extents require native item/path and stdout joins; other shell forms unsupported. |
| OpenCode | OverExploring | Y | Y | N | N | N | Native `OpenCodeSqliteV2` retained text task context and bound read requests. Within-file targets require pinned numbered result extents; no verification or savings. |
| Pi | OverExploring | Y | Y | N | N | N | Native 0.84.4 core V3 retained task context and bound Read requests; within-file targets require exact text/clipping extents; continuation is not whole-file access. |
| Cursor | OverExploring | N | N | N | N | N | Complete task history and observed-result support are unavailable. |
| Antigravity | OverExploring | N | N | N | N | N | Complete task history and observed-result support are unavailable. |
| Claude Code | ScopeCreep | Y | Y | N | N | N | Native CLI 2.1.278 retained root with explicit human markers; SDK/meta/synthetic text and tool permission do not authorize scope; original retention not proved. |
| Codex | ScopeCreep | Y | Y | N | N | N | Native 0.160.1 pinned retained root; qualified single-local environment context is non-authorizing; unknown-origin answers and skill selections cannot approve scope. |
| OpenCode | ScopeCreep | Y | Y | N | N | N | Native `OpenCodeSqliteV2` current retained root scope only. Requests use selected task/scope passages with explicit partial limits; compact requests need earlier user task context. Later recorded approval invalidates stale findings. Invalid selected dependencies or work that cannot fit remain unassessed. Original historical retention is not proved. |
| Pi | ScopeCreep | Y | Y | N | N | N | Native 0.84.4 core V3 linear retained root; exact user argument ranges stay separate from skill documents; extension answers do not prove human origin. |
| Cursor | ScopeCreep | N | N | N | N | N | Current retained root scope proof is unavailable. |
| Antigravity | ScopeCreep | N | N | N | N | N | Current retained root scope proof is unavailable. |

Cursor and Antigravity M/K are current-inventory targets, not historical
exposure claims. Pi S remains limited to reviewed example-extension evidence.
M/B/K estimates can use measured replicated tokens when available and otherwise
the bounded finding-rate fallback. The fallback is not measured token or price
evidence.

## Second-Tier Product Coverage

GitHub Copilot, Cline, Kiro, Amp, Devin, and Oh My Pi remain second-tier in
product documentation. This defers no implemented parser, finding path, prompt,
Auto Fix, verification, or burn-estimate behavior. The accepted Copilot CLI v1
event and schema-v7 request-store bundle supports S/O results. It does not
support D because the production reader has no request-depth evidence. Oh My Pi
supports D/T/O results from the shared Pi core. Overdepth reads the largest
single request, so an in-file abandoned branch cannot change the context of
another request. It does not support S, because OMP subagents live in sibling
files this reader does not open. It has no inventory, no remediation prompt,
and no clean result. The other current source and check limits remain the
source inventory and coverage matrix above.

## Evidence Boundaries

Remote session copies retain the accepted Claude Code or Codex transcript
contract; they do not acquire broader coverage by arriving over SSH. A
per-session finding requires sufficient accepted copied evidence. Missing
companions and historical configuration remain unavailable or partial, never
clean. This computer's configuration and provider account do not enrich remote
evidence. Bounded discovery does not establish a complete remote inventory.
Rejected or incomplete exports keep the previous cached generation; a partial
host scan does not establish fresh evidence for the rejected sessions. Skipping
expired listing candidates does not prove a complete companion roster; Codex
exports still require bounded origin discovery for older linked children.
Remote sessions do not contribute to the local global-check report,
Overview, quota attribution, or live HUD. Local path actions, Auto Fix, and
remediation/watch enrollment reject remote origins at the backend boundary.
The product and remediation matrices below describe native supported contexts;
they do not grant remote editing or verification. See
[remote sessions](remote-sessions.md) for the supported host and agent limits.

Lifecycle provider sweeps use same-turn published provider/model evidence and
keep the harness and inferred model vendor separate from the recorded route.
Missing or custom routes do not prove direct provider activity. These display
signals do not authorize a finding, a clean result, remediation, or historical
spend attribution; all check-specific route and API requirements below remain
unchanged. See [`session-lifecycle-events.md`](session-lifecycle-events.md#scoped-sweep-evidence).

Burn checks use only sessions admitted by the repository scan gate. A session
needs a resolvable Git repository CWD, either recorded or inferred from
transcript paths below a parent-folder CWD. Disabled roots and their linked
worktrees are excluded before evidence processing; missing or unresolved CWDs
are unavailable, never clean. When "Include folders without git" is on, a
session outside a repository enters the index with no project root. Its findings
can still show, but its agent's resource inventory counts as limited, so that
agent gets no clean result, and remediation has no repository target to edit.

No current session reader proves a full historical resource inventory. The
legacy per-session M/B/K rules deny `Clean`, even when a nested observed-resource
map is complete. The desktop target assessment can report clean only when every
applicable current inventory scan and positive-use input is complete and within
its bounds. A scoped finding requires complete coverage of that observed subset,
calls, and eligible activity. An unrelated partial resource group does not block it.
Detector-level absence never verifies an M/B/K remediation. Unavailable,
partial, wrong-source, wrong-agent, wrong-scope, truncated, and non-applicable
evidence does not verify the target.

The native desktop advisory inventory is separate from these per-session
detector rules. It enumerates bounded standard current resources for Claude
Code, Codex, Cursor, Copilot, Cline, OpenCode, Kiro, Amp, Antigravity,
Devin/Windsurf, and Pi and can merge current indexed resource observations. Its result
contains logical names, state, scope, provenance, and
limits only. Skill candidates can include a proportional token estimate for the
frontmatter `title` or `name` plus `description`. The estimate excludes the
skill body. The inventory contains no physical path or selector.

The desktop 30-day report reduction now creates a separate target-based M/B/K
assessment. One key contains the exact agent, resource kind, normalized name,
and global or canonical repository scope. Any exact positive use in that scope
suppresses the key. Remaining candidates become one target each, not one target
per session. The assessment records target totals and at most three supporting
sessions per target. The shipped Checks DTO and action command use this
assessment for M/B/K category counts, agents, status, burn estimates, and named
target rows. They do not use the older session-based M/B/K rows.

Positive use can come from tool counts, invoked loaded sources,
catalog-backed tool definitions, or persisted initial-context source rows.
Observed positive facts remain valid inside partial evidence. Partial or
unsupported facts, malformed or dynamic applicable inventory, unknown scope,
ambiguous aliases, failed scans, and repository, context, use, directory, or
target caps block clean. They do not remove a known finding from an unrelated
kind, scope, or agent. Disabled current resources do not become candidates.

Global candidates share use only across the same agent. Project candidates
share use only inside one canonical accessible repository. A raw call with no
origin uses a same-name project candidate in its repository before a global
candidate. Unknown-origin indexed resources do not become scoped targets.
Same-name resources in separate scopes and repositories stay separate.

Resource burn estimates use the report's existing total-token denominator and
rounding. Skill listing tokens use the existing proportional `chars / 4`
estimate and replicate across applicable assistant turns. MCP estimates use
only measured indexed definition tokens. Claude Code and Codex built-in tools
reuse measured catalog definitions. Only optional specialized tools can become B
targets; required shell, read, write, edit, search, and subagent tools remain
measured but never become findings. OpenCode 1.2.15 and Pi 0.52.12 use pinned
default catalog captures. A matching positive use removes the target and its
estimate. A missing definition, missing denominator, cap, truncation, or
arithmetic failure uses the detector's bounded finding-rate fallback instead
of presenting no percentage. Measured token attribution always replaces that
fallback. The fallback is an estimated workload share, not measured tokens or
price data. It stays within the 0% to 100% display range.

B tool eligibility is a product safety policy, not a claim that vendors make
other tools impossible to disable. The only eligible names are Claude Code
`WebSearch`, `WebFetch`, `Workflow`, `ReportFindings`, and `ScheduleWakeup`, Codex `web_search`, and OpenCode `websearch` and
`webfetch`. Pi, Cursor, Copilot, Cline, Kiro, Amp Code, Antigravity, and
Windsurf have no B target. Shell, read, write, edit, search, task, agent, and
subagent tools remain measured but never become B targets for any agent.
Every displayed resource target has at least one supporting failed session. A
resource without that session evidence makes the category unavailable instead
of creating a target without a session to open.

The reviewed primary sources are Claude Code [permissions](https://code.claude.com/docs/en/permissions)
and [tools](https://code.claude.com/docs/en/tools), Codex [configuration
reference](https://developers.openai.com/codex/config-file/config-reference),
OpenCode [permissions](https://opencode.ai/docs/permissions/) and
[tools](https://opencode.ai/docs/tools/), Pi [settings](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/settings.md#tools),
and Cursor [SDK tool restrictions](https://cursor.com/docs/sdk/python#restricting-the-toolset).
These sources show that several core tools are configurable. They do not
identify them as optional, so antiburn does not suggest disabling them.
Claude Code documents `Workflow` as a separate dynamic-workflow tool and
documents independent feature controls in its [workflows](https://code.claude.com/docs/en/workflows#turn-workflows-off)
and [settings reference](https://code.claude.com/docs/en/settings-reference#disableworkflows).

Provider matching accepts exact case-insensitive names and the reviewed call
forms: Claude Code and Codex `mcp__<server>__<tool>`, OpenCode
`<server>_<tool>`, and `pi-mcp-extension` 1.5.0 default
`mcp_<sanitized-server>_<tool>`. Claude Code and Codex accept a unique bare
suffix for namespaced skills and the final segment of a catalog-backed built-in
alias. Ambiguous skill aliases suppress every possible matching target and
block clean. OpenCode and Pi MCP names can be ambiguous because the server and tool share one underscore
delimiter. Such a call suppresses every possible matching server finding and
blocks clean, but it does not count any server as used. OpenCode and Pi use exact
skill and built-in names.

The accepted current shapes are Claude standard user/project `mcpServers`,
standard `.claude/skills`, `skillOverrides`, and exact permission controls;
Codex trusted layered `mcp_servers` with omitted `enabled` treated as enabled,
plus standard `.agents/skills` and compatibility `.codex/skills`; OpenCode
JSON/JSONC direct and `mcp.servers` maps, standard skill roots, Boolean `tools`,
and object or array permission controls; and Pi `defaultTools`, standard skill
roots, explicit non-pattern skill directories, and pinned
`pi-mcp-extension` 1.5.0. The Pi package manifest must name version 1.5.0 and
`./src/index.ts`; the reviewed producer commit is
`8a01fc53f3289d2e8eb492d67ba45cd84d64e7f2`. Runtime, managed, remote, plugin,
pattern, lazy Pi MCP, malformed, unsafe, capped, conflicting, and partial
indexed sources remain explicit clean-result limits.

Inventory files are Cursor `.cursor/mcp.json`; Copilot
`~/.copilot/mcp-config.json`, `.mcp.json`, and `.github/mcp.json`; Cline MCP
settings; Kiro `.kiro/settings/mcp.json`; Amp settings; Antigravity
`mcp_config.json`; and Devin/Windsurf MCP settings. Their documented skill roots
are scanned in global and project scope. Where supported, global
`~/.agents/skills` is scanned with the vendor-specific root, and duplicate
same-agent, same-scope identities are merged. These inputs are current state,
never historical exposure, and malformed, dynamic, plugin, unsupported, or
capped inputs block clean.

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

| Source                                                                    | Checks     | Implemented contract and remaining limit                                                                                                                                                                                                                                                               |
| ------------------------------------------------------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Claude and Codex                                                          | D, O       | Direct request depth and timed model use reach checks independently of token-accounting policy. Clean needs all required session facts and reviewed model state. Unknown models are not automatically current.                                                                                         |
| Claude and Codex                                                          | T, F       | Request-level model, effort, speed, and route observations are evaluated together. Codex preserves explicit provider/control changes and inherited fork state; child controls retain delegated scope. Missing eligible signals or unreviewed routes deny clean.                                        |
| Claude                                                                    | S          | Exact `Task`/`Agent` call IDs join unique sidecar `toolUseId` claims to actual child models and the model on the parent call. Missing, duplicate, malformed, mismatched, or nested-parent claims remain partial. Requested model aliases and directory ancestry are not proof.                         |
| Codex                                                                     | S          | Owned `spawn_agent` records and discovered child rollouts provide delegation and actual models. Incomplete child evidence cannot prove clean.                                                                                                                                                          |
| Claude                                                                    | M, K       | Observed MCP injection plus exact server calls and full skill-document injection plus invocation identity support scoped findings. Skill listings alone do not. The observed subset is not a historical inventory.                                                                                     |
| Codex                                                                     | M          | Accepted completed client `tool_search_output` namespace records expose exact MCP server identities. Complete observed exposure and calls support scoped findings without a full-inventory capability. Ambiguous or incomplete search results do not establish injection.                              |
| Claude and Codex                                                          | B          | The existing harness-version/model catalog path supports scoped definition findings with complete calls. Deferred, situational, and zero-cost definitions are excluded. Catalog resolution does not prove every historical enabled tool or exposure change; it cannot justify a whole-inventory claim. |
| Codex                                                                     | K          | Selected full skill documents reach observed injection/invocation evidence. Listings remain availability only. Selected documents do not establish unused listing overhead or full inventory coverage.                                                                                                 |
| OpenCode                                                                  | S          | Native `task` metadata identifies the child session and model; ancestry and the child's assistant model must agree. The parent model comes from the task request. A bare `subtask`, fork, or `parent_id` relation is insufficient.                                                                     |
| OpenCode                                                                  | K          | Complete native selected-skill results preserve full identity as injected and invoked. Truncated, compacted, empty, or invalid result wrappers do not prove full injection. This is observed selected-skill support, not an unused-listing finding or complete inventory.                              |
| OpenCode                                                                  | T, M, B, F | Confirmed unsupported for the reviewed sources: no historical effort map, model-facing resource inventories, or effective speed tier. The reader does not retain a variant as effort. Variant labels, current configuration, and tool registries cannot substitute.                                    |
| Pi                                                                        | T          | `EffortSemantics::AgentSelectedPolicy` evaluates the saved agent-selected thinking level, not translated provider effort. Above-cap findings require positive usage on the same model/effort observation. Missing usage, levels/routes, and unknown models fail closed; provider overrides are not guessed. |
| Pi                                                                        | S          | Existing output from the official subagent example extension supplies nested `toolResult` messages, exact native call/worker identity, and actual models. This is finding-only. Arbitrary extensions, fork ancestry, requested aliases, and a nonpremium observed worker cannot establish clean.       |
| Pi                                                                        | M, B, K, F | Confirmed unsupported for the reviewed sources. Tool calls and bounded skill invocation identity do not establish historical resource exposure or speed. No alternative local proof was identified.                                                                                                    |
| Quota pressure and provider incidents sit outside the nine-code check     |
| contract (FR-15): neither has a row in the Checks table above, and each   |
| reports only when transcripts carry its own evidence. `CodexRolloutJsonl` |
| and `ClaudeJsonl` both supply quota and provider incidents.               |

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

A Claude quota incident also carries the limit family and the reset time the
record states in its message text. A `session limit` text gives
`RollingWindow` and a `weekly limit` text gives `Weekly`; a text neither
phrase matches stays `RateLimit`, because `apiErrorStatus` alone still proves
the refusal. The stated reset becomes a `QuotaResetClock` — an hour, a minute,
and the named zone — not an instant: the engine holds no zone database, so the
application resolves the clock. The reader keeps the two parsed values and
drops the text; no message text is stored. Evidence written before this field
existed deserializes with no clock.

Every other status or `error` value is ignored, including `error: "unknown"`
with no status (Claude's connection-refused case) and every 4xx other than 429. `ProviderIncidentKind::Connection` is Codex-only: no Claude field
reviewed so far identifies a connection failure without reading message
text.

Clean or absence is never claimed from either group for any source: each
section is not assessed without at least one observed incident of its own
kind, per FR-15's one condition.

Maintainer confirmation (2026-09-14): extend provider incidents with
`ServerError` and `Connection`, map Codex's remaining transport/server
`codex_error_info` codes, and add Claude `isApiErrorMessage` records as a
new quota/provider incident source. Reviewed passive alternatives:

- Classifying Claude errors from `content[].text` — rejected: free text and
  unpinned. The reader parses the limit family and the reset clock from an
  `isApiErrorMessage` text and keeps neither the text nor any other message
  text.
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

| Claude, Codex, OpenCode, Pi | C | Durable request provider/API fields and the compatible-request query select reviewed cache-write or uncached-input accounting. A finding additionally needs a same-route hit/miss/recovery episode after the route's reviewed cache lifetime: Claude Code uses its configured one-hour default; Pi and OpenCode Anthropic routes use five minutes unless an earlier write records one-hour TTL evidence, which carries across hits and refreshes at hit request start; reviewed OpenAI routes use 30 minutes. OpenCode continuity uses validated consecutive order and distinct message IDs because `parentID` identifies the answered user, not the predecessor. Main-thread identity, order, token classes, model, route, and compaction boundaries constrain pairs. Codex pairs `token_usage_record` with equivalent `token_count` usage by per-response fields; matching cumulative fields permit delayed exact copies. Unknown or incompatible segments prevent findings and clean results. Google cache policy remains unreviewed. |
| OpenCode | C | Both accepted export and SQLite shapes use validated ordered history. `parentID` identifies the user being answered, not the predecessor. Missing wrappers/timestamps, duplicate or out-of-order messages, and unresolved forks prevent complete history. CoreV2 `session_message` is not the existing SQLite table contract. |
| Cursor | D, O | O retains direct timed-model findings; the source gate denies clean on every surface. D remains unavailable because the current reader does not emit request-usage evidence. Synthetic source-gate tests do not establish native parsing support. |
| Cursor | T, S, M, B, K, F, C | Broader surface characterization is deferred. Current settings, relations, inventories, and cache evidence remain partial, unknown, or unsupported as listed; no new parity claim is made. |
| Antigravity | D, O | Brain/cascade steps and native SQLite preserve direct usage/model findings where present. Missing model/time is not filled from an earlier step or an invented database timestamp. Private identity, enum, and completeness gaps deny clean. |
| Antigravity | T, S, M, B, K, F, C | Confirmed unsupported in the reviewed native evidence. Token classes do not establish compatible request linkage or cache cause. Runtime descriptors and unproved relationship sidecars do not establish persisted delegation, controls, or resource exposure. Workspace chat remains uncharacterized. |

M automatic remediation accepts only one named, enabled target with indexed
provenance and an exact current editor resolution. Codex resolves exactly one
active trusted project or global `mcp_servers.<name>` table. Claude resolves
exactly one standard project or global MCP source and adds only its matching
deny rule to one same-scope existing settings file. OpenCode has an exact
`enabled = false` editor, but M remains prompt-only when indexed provenance or
one exact current control is unavailable.
Antigravity has no public source and precedence proof for one persisted disable
field. Cursor MCP remediation is unavailable and never reads a private toggle
store or invokes a CLI command.

Cache churn selects its policy from `RepeatedContextAccounting`, not from the
agent or the session's dominant model. `CacheWrite` uses the reviewed Claude
family policy. `UncachedInput` uses the reviewed OpenAI family policy. This rule
also applies to mixed-family sessions. A cache-churn cause names a model from the
same accounting family; it does not use an unrelated dominant model. Repeated
input totals remain available even when there is no actionable episode. A
continuous-activity hit/miss/recovery episode is informational and leaves a
high-ratio result not assessed. A finding requires recovered cache use after a
supported route-specific user inactivity interval. Unknown routes, missing
timestamps, broken identity, compaction, model changes, and unrecovered misses
cannot establish that finding or a clean result.

Old-model causes remain separate by provider, API, observed model, and reviewed
replacement. Token-burn percentages are unknown when a required price or the
total-token denominator is absent. The estimator does not use a 10 percent
fallback and does not force a positive minimum.

## Passive Verification

Every T, O, or F finding with supported positive verification from a winning
`Ready` evidence publication can create one passive attempt, up to 100 attempts
per publication. Candidate filtering applies the detector, agent, source-format,
physical-target, and non-resource requirements before bounded selection. The
selection is fair across all nine detectors. V45 does not backfill old published
rows. The immutable boundary is the publication time in milliseconds, not the
session time. Replay reuses an active target. After recurrence, a later
publication can create a new attempt. A later action keeps a separate action
attempt for the same target; it does not replace the passive attempt or move its
boundary. Attempt creation, dirtying, evidence publication, and fenced row
replacement share the winning transaction.

The table below is the exact implemented positive-proof matrix. `Supported`
means the current backend can verify a fixed transition. `Unavailable` means it
does not enroll a passive attempt and cannot prove the initial fix from the
accepted passive evidence. An explicit action can store
`verificationUnavailable`. Source coverage from the main matrix still applies.

| Check | Claude Code | Codex       | OpenCode    | Pi          | Cursor      | Antigravity | Proof or blocker                                                                                                                                                                              |
| ----- | ----------- | ----------- | ----------- | ----------- | ----------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | The finding identity is one historical session. A later session is not positive proof that the original session changed.                                                                      |
| T     | Supported   | Supported   | Unavailable | Supported   | Unavailable | Unavailable | A complete later assessment plus an explicit same-route, same-model lower control proves the transition. Pi proves only its agent-selected policy.                                            |
| S     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | A later worker or call has a different identity. No accepted source records a durable worker-setting transition.                                                                              |
| M     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed resource subsets cannot prove that a server was removed or disabled.                                                                                                                 |
| B     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed resource subsets cannot prove that a tool was removed or disabled.                                                                                                                   |
| K     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Observed resource subsets cannot prove that a skill was removed or disabled.                                                                                                                  |
| O     | Supported   | Supported   | Supported   | Supported   | Unavailable | Unavailable | The strict verifier requires actual replacement use on the same publication-attributed physical target, scope, provider, and API. Cursor and Antigravity have no physical target attribution. |
| F     | Supported   | Supported   | Unavailable | Unavailable | Unavailable | Unavailable | A complete later assessment plus an explicit same-route, same-model standard-tier delegated request proves the transition.                                                                    |
| C     | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | Unavailable | A later request pair is not the same session-route target and does not prove a durable cache-policy transition.                                                                               |

Truncated assessment sets, sessions that start at or before the boundary,
missing controls, changed detector or catalog policy, stale projections, and
unsupported source contracts return verification unavailable or continue
watching. They never become fixed through generic absence. For T and F, one
complete later session must contain the exact positive control for the same
target. For O, one later session must contain actual replacement-model use for
the same attributed target. Report-level absence and historical counts do not
verify a fix. A fixed supported target recurs only on a later exact positive
observation. Prior contributions end at the recurrence boundary and remain
durable.

The engine verifier is fail-closed for M/B/K. Historical session subsets are
always unavailable, whether they contain or omit the target. Only a complete,
bounded later current inventory with matching agent, source, scope, and use
coverage could verify absence or recurrence. No production desktop path supplies
that inventory to a watch today, so reachable M/B/K verification is unavailable.
Their stored resource selector cannot use model proof, establish report clean,
or create verified savings.

A successful M/B/K Auto Fix retains only the crash-safe write record with
`verificationUnavailable`. The result is `applied_verification_unavailable`, not
`applied_awaiting_verification`, and the UI does not move the check into the
awaiting-verification group.

## Automatic Editor Support

`Auto Fix` means the backend can bind a finding to one effective physical
setting, prepare a reviewed edit, and recover an uncertain write. Resource Auto
Fix additionally requires indexed provenance and an exact current resource,
scope, value, and physical key. `Prompt only` means the existing bounded prompt
can describe the finding, but the backend cannot prove one safe physical edit.
Source versions mean the accepted source shapes in the source inventory. No row
claims every historical agent release.

Each Auto Fix edits one winning control. An inherited value selects its global
or user control. A project target requires the exact explicit project setting or
resource; the editor never creates a project config. Scalar edits are not
batched across active layers. Findings from multiple projects that resolve to
the same global control form one target, and prepare and apply revalidate every
grouped project context. Model and reasoning edits remain pinned to the
publication-time scope and physical target.

| Agent                            | Operation                   | Result                        | Reason or exact limit                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| -------------------------------- | --------------------------- | ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Claude Code                      | Model replacement           | Auto Fix                      | `ClaudeJsonl` only. The publication must bind the observed main-loop model to an existing effective `model` setting.                                                                                                                                                                                                                                                                                                                                                                                |
| Claude Code                      | Reasoning effort            | Auto Fix                      | `ClaudeJsonl` T findings only. The publication must bind the observed level to an existing effective top-level `effortLevel` or model-specific `modelSettings.<model>.effortLevel`. The reviewed replacement is `medium`.                                                                                                                                                                                                                                                                           |
| Claude Code                      | Fast mode                   | Auto Fix                      | `ClaudeJsonl` F findings only. The finding needs explicit fast-tier evidence and the current winning existing `fastMode` value must be `true`; the editor writes `false` to that one control so an inherited global `true` cannot become effective. No publication-time config attribution is required.                                                                                                                                                                                             |
| Claude Code                      | Named subagent model        | Auto Fix                      | `ClaudeJsonl` S findings only. The current scan must locate exactly one named Markdown agent whose frontmatter `model` equals the observed worker model.                                                                                                                                                                                                                                                                                                                                            |
| Claude Code                      | MCP control                 | Auto Fix or prompt only       | Auto Fix requires indexed provenance, one current standard project or global MCP source, and one exact same-scope existing settings target. Otherwise the named target remains prompt-only.                                                                                                                                                                                                                                                                                                         |
| Claude Code                      | Skill control               | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one current standard `SKILL.md` winner. The editor writes only `skillOverrides.<name> = "off"`. Inventory-only or ambiguous targets remain prompt-only.                                                                                                                                                                                                                                                                                                    |
| Codex                            | Model replacement           | Auto Fix                      | `CodexRolloutJsonl` only. The publication must bind the observed main-thread model to an existing effective top-level `model`. Project edits require an explicit `trust_level = "trusted"` entry and a repository-root cwd.                                                                                                                                                                                                                                                                         |
| Codex                            | Reasoning effort            | Auto Fix                      | `CodexRolloutJsonl` T findings only. The publication must bind the observed level to an existing effective top-level `model_reasoning_effort`. The reviewed replacement is `medium`.                                                                                                                                                                                                                                                                                                                |
| Codex                            | Fast service tier           | Auto Fix                      | `CodexRolloutJsonl` F findings only. The finding needs explicit fast-tier evidence and the current winning existing `service_tier` must be `fast`; the editor changes it to reviewed `standard`. No publication-time config attribution is required.                                                                                                                                                                                                                                                |
| Codex                            | Named subagent model        | Auto Fix                      | `CodexRolloutJsonl` S findings only. The current scan must locate exactly one named agent TOML file whose `model` equals the observed worker model.                                                                                                                                                                                                                                                                                                                                                 |
| Codex                            | MCP enablement              | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one exact active trusted project or global `mcp_servers.<name>` table. Inventory-only, duplicate, or untrusted targets remain prompt-only.                                                                                                                                                                                                                                                                                                                 |
| Codex                            | Skill enablement            | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one current trusted standard `SKILL.md` winner with one matching `skills.config.<name>.enabled` control. Other targets remain prompt-only.                                                                                                                                                                                                                                                                                                                 |
| OpenCode                         | Model default               | Auto Fix                      | `OpenCodeJsonl` and `OpenCodeSqliteV2` O findings only. Direct `openai`, `anthropic`, and `google` provider IDs use their reviewed native API when OpenCode omits it. Publication must bind the observed `provider/model` route to the effective merged `model` value. Dynamic, remote, agent, mode, and managed overrides are rejected.                                                                                                                                                            |
| OpenCode                         | Named subagent model        | Auto Fix                      | S findings only. The current scan must locate exactly one named Markdown agent whose frontmatter `model` equals the observed worker model. Variant-only workers remain unavailable.                                                                                                                                                                                                                                                                                                                 |
| OpenCode                         | Reasoning control           | Unavailable                   | The accepted sources have no historical effort map. A variant label is not an effective reasoning control.                                                                                                                                                                                                                                                                                                                                                                                          |
| OpenCode                         | MCP control                 | Auto Fix or prompt only       | Current inventory can create a named target. Auto Fix requires indexed provenance and one exact effective V2 MCP control; other targets remain prompt-only.                                                                                                                                                                                                                                                                                                                                         |
| OpenCode                         | Skill control               | Auto Fix or prompt only       | Auto Fix requires indexed provenance and one current standard `SKILL.md` winner. The editor appends one V2 `skill` deny with that exact resource. Other targets remain prompt-only.                                                                                                                                                                                                                                                                                                                 |
| Pi                               | Model and provider default  | Auto Fix                      | `PiV3Jsonl` O findings only. Publication must bind the observed `provider/model` route to an existing paired `defaultProvider` and `defaultModel` setting.                                                                                                                                                                                                                                                                                                                                          |
| Pi                               | Thinking level              | Auto Fix                      | `PiV3Jsonl` T findings only. Publication must bind the saved agent-selected level to an existing route-specific `modelThinkingLevels` entry or `defaultThinkingLevel`. The reviewed replacement is `medium`.                                                                                                                                                                                                                                                                                        |
| Pi                               | MCP or skill control        | Prompt only                   | Current inventory can create named M/K targets. Pi core built-ins never become B targets. The production policy has no exact M/K editor binding. No `SKILL.md` file is changed.                                                                                                                                                                                                                                                                                                                     |
| Pi, Cursor, Antigravity          | Named subagent model        | Unavailable                   | Pi extension output and Cursor or Antigravity findings do not bind one effective persisted worker-model selector.                                                                                                                                                                                                                                                                                                                                                                                   |
| Claude Code, Codex, OpenCode, Pi | Session compaction          | Auto Fix for D only           | The current project or global config must contain a supported disabled compaction flag or a numeric limit above the finding depth cap. The editor enables the flag or lowers that limit to the cap. Ordinary session growth, enabled controls, fixed instructions, runtime overrides, and unsupported schemas remain unavailable.                                                                                                                                                                   |
| Antigravity                      | Model or documented setting | Prompt only for O findings    | Accepted sources can retain direct model use, but no accepted IDE or CLI source binds it to one effective documented physical setting.                                                                                                                                                                                                                                                                                                                                                              |
| Antigravity                      | MCP control                 | Unavailable                   | The reviewed native evidence has no MCP exposure or effective-control contract. IDE and CLI configuration cannot be interchanged.                                                                                                                                                                                                                                                                                                                                                                   |
| Claude Code                      | Built-in tool               | Auto Fix for optional B tools | One exact observed tool must have a matching canonical permission name. A project target requires that bare name in its exact `permissions.allow` array. Otherwise the inherited control is global. The editor can add the bare deny to existing global settings or create the missing global settings file. `Bash`, `Edit`, `Read`, and `Write` remain measured but cannot receive Auto Fix or a targeted disable prompt. Wildcards, scoped rules, and general permission changes are unavailable. |
| Codex                            | Built-in tool               | Unavailable                   | The documented `apps.<id>.tools.<tool>.enabled` control applies to an app tool, not one built-in tool identity.                                                                                                                                                                                                                                                                                                                                                                                     |
| OpenCode                         | Built-in tool               | Prompt only                   | Current inventory can create an allowlisted optional target, but the production policy does not expose a built-in automatic edit.                                                                                                                                                                                                                                                                                                                                                                   |

The prompt matrix below comes from `remediation/prompts.rs`. A `Yes` still needs
one finding that passes the source coverage gates above.
The check-level Copy action can return bounded generic text for one or more
selectable current targets. It returns no prompt when no target is selectable.
Every returned prompt has one durable `ABR-` reference and each selected target
has a durable action attempt.

| Agent and source                                                                                          | D   | T   | S   | M   | B   | K   | O   | F   | C   | I   |
| --------------------------------------------------------------------------------------------------------- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Claude Code, `ClaudeJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| Codex, `CodexRolloutJsonl`                                                                                | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| OpenCode, `OpenCodeJsonl` or `OpenCodeSqliteV2`                                                           | Yes | No  | Yes | Yes | Yes | Yes | Yes | No  | Yes | Yes |
| Pi, `PiV3Jsonl`                                                                                           | Yes | Yes | Yes | Yes | No  | Yes | Yes | No  | Yes | No  |
| Cursor, `CursorCliAgentJsonl`                                                                             | No  | No  | No  | No  | No  | No  | No  | No  | No  | Yes |
| Antigravity, `AntigravityJson`, `AntigravityBrainJsonl`, `AntigravityCascadeJson`, or `AntigravitySqlite` | Yes | No  | No  | No  | No  | No  | Yes | No  | No  | No  |

### Source-Format Remediation Matrix

This exact matrix lists every `SourceFormat` once. Check-code lists are typed
sets: `None` or slash-separated codes from D/T/S/M/B/K/O/F/C/I/SkillOpportunities/OverExploring/ScopeCreep with no duplicates.
The rows describe reachable remediation, so a production recommendation without
a reachable finding is not listed. Auto Fix columns describe production policy;
each operation still needs an exact target binding at runtime.

| `SourceFormat`                 | Prompt checks     | Model Auto Fix | Reasoning Auto Fix | Other Auto Fix checks | Verification checks |
| ------------------------------ | ----------------- | -------------- | ------------------ | --------------------- | ------------------- |
| `ClaudeJsonl` | D/T/S/M/B/K/O/F/C/I/SkillOpportunities/OverExploring/ScopeCreep | O | T | D/S/M/B/K/F | T/O/F |
| `CodexRolloutJsonl` | D/T/S/M/B/K/O/F/C/I/SkillOpportunities/OverExploring/ScopeCreep | O | T | D/S/M/K/F | T/O/F |
| `OpenCodeJsonl`                | D/S/M/B/K/O/C     | O              | None               | D/S/M/K               | O                   |
| `OpenCodeSqliteV2`             | D/S/M/B/K/O/C/I/SkillOpportunities/OverExploring/ScopeCreep | O              | None               | D/S/M/K               | O                   |
| `PiV3Jsonl` | D/T/S/M/K/O/C/I/SkillOpportunities/OverExploring/ScopeCreep | O | T | D | T/O |
| `OmpV3Jsonl`                   | None              | None           | None               | None                  | None                |
| `CursorJsonl`                  | O                 | None           | None               | None                  | None                |
| `CursorCliAgentJsonl`          | O/I               | None           | None               | None                  | None                |
| `CursorCliStoreDb`             | O                 | None           | None               | None                  | None                |
| `CursorChatStoreDb`            | O                 | None           | None               | None                  | None                |
| `CursorIdeComposer`            | O                 | None           | None               | None                  | None                |
| `CursorLegacyChatJson`         | None              | None           | None               | None                  | None                |
| `AntigravityJson`              | D/O               | None           | None               | None                  | None                |
| `AntigravityBrainJsonl`        | D/O/I             | None           | None               | None                  | None                |
| `AntigravityCascadeJson`       | D/O               | None           | None               | None                  | None                |
| `AntigravityWorkspaceChatJson` | None              | None           | None               | None                  | None                |
| `AntigravitySqlite`            | D/O               | None           | None               | None                  | None                |
| `CopilotCliJsonl`              | None              | None           | None               | None                  | None                |
| `CopilotIdeChatJson`           | None              | None           | None               | None                  | None                |
| `ClineSessionJson`             | None              | None           | None               | None                  | None                |
| `ClineMessagesContractV1`      | None              | None           | None               | None                  | None                |
| `KiroSessionJson`              | None              | None           | None               | None                  | None                |
| `KiroChat`                     | None              | None           | None               | None                  | None                |
| `KiroCliV2Bundle`              | None              | None           | None               | None                  | None                |
| `KiroCliV3Bundle`              | None              | None           | None               | None                  | None                |
| `KiroChatSaveExport`           | None              | None           | None               | None                  | None                |
| `AmpThreadJson`                | None              | None           | None               | None                  | None                |
| `AmpFileChanges`               | None              | None           | None               | None                  | None                |
| `WindsurfWorkspaceJson`        | None              | None           | None               | None                  | None                |
| `WindsurfMirrorJson`           | None              | None           | None               | None                  | None                |
| `WindsurfCascadeProtobuf`      | None              | None           | None               | None                  | None                |
| `DevinLocalSqlite`             | None              | None           | None               | None                  | None                |
| `Uncharacterized`              | None              | None           | None               | None                  | None                |

| Scope and environment              | macOS       | Linux       | Native Windows        | WSL         |
| ---------------------------------- | ----------- | ----------- | --------------------- | ----------- |
| Global model or reasoning setting  | Auto Fix    | Auto Fix    | Read attribution only | Unavailable |
| Project model or reasoning setting | Auto Fix    | Auto Fix    | Read attribution only | Unavailable |
| Session or worker setting          | Unavailable | Unavailable | Unavailable           | Unavailable |

The macOS and Linux implementation rejects unreviewed runtime overrides,
managed or system configuration, Codex profiles, untrusted workspaces,
unsupported precedence, unsupported missing files, duplicate definitions, malformed data,
files above 256 KiB, non-regular files, target or ancestor symlinks, and wrong
Unix owner or group. Apply re-resolves precedence, checks the original file
identity and bytes, writes an exclusive same-directory temporary file,
preserves mode and owner/group, syncs it, atomically replaces the target, syncs
the directory, and performs typed readback. A changed target conflicts without
retargeting. The approved missing-file exception creates only global Claude
settings for an eligible optional built-in tool. It uses exclusive creation,
safe parent directories, directory sync, and exact-byte readback.

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

| Agent       | Reviewed persisted contract                                                                                                                     | Attribution decision                                                                                                                                                                                                           |
| ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Claude Code | Managed settings override CLI, local, project, and user files. `ANTHROPIC_MODEL` has per-key precedence over `model`.                           | Model and reasoning use only the attributed existing user, project, or local selector. Current resolution must match its saved scope and physical target. Managed, CLI, host, or environment winners are unavailable.          |
| Codex       | CLI overrides trusted project files from repository root through CWD, then an explicitly selected profile, user config, and system config.      | Model and reasoning resolve every trusted nested project file, then require the saved scope and physical target. A selected profile, runtime override, system or managed config, or untrusted workspace is unavailable.        |
| OpenCode    | Remote, global, custom, nested direct project, `.opencode`, inline, managed file, then MDM sources merge in that order.                         | Model attribution reads the reviewed global and nested project JSON/JSONC subset, then pins Auto Fix to the saved scope and physical target. Remote, custom, inline, managed, dynamic, agent, and mode inputs are unavailable. |
| Pi          | Trusted project `.pi/settings.json` deep-merges over global settings. CLI provider/model/thinking and session-directory inputs take precedence. | Model and reasoning use the attributed existing winning project or global selector and require its saved scope and physical target. Agent-directory, CLI, or split provider/model inputs are unavailable.                      |
| Cursor      | CLI JSON, CLI permissions, MCP JSON, IDE settings, and team controls are separate contracts.                                                    | Unavailable. No accepted Cursor session source proves that one persisted CLI or IDE setting caused the observed model behavior.                                                                                                |
| Antigravity | Documented global and workspace MCP files do not define model-setting precedence for every IDE and CLI surface.                                 | Unavailable. Accepted session sources do not bind a model or setting to one physical control.                                                                                                                                  |

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

The stored estimated-savings value is the target's pre-remediation opportunity;
it is not recent usage. The aggregate-savings read returns only exact current
cycles that remain fixed, retain a fixed verification result and verified
boundary, match their stored finding snapshot, are enabled, and are not actively snoozed.
The renderer additionally requires a visibly Passed detector and hides Savings
when no eligible cycle remains. Confirmed savings alone can use eligible
post-verification sessions.

The remediation backend lists current displayable findings even when no action
is safe. Prompt support follows the source and check limits in this document.
T and F prompt watches require fresh, complete post-boundary assessment and the
exact positive control described in the matrix before a fix can verify.
Old-model watches are stricter: only actual old or replacement model use
attributed to the same publication-time effective physical target, scope,
provider, and API can change the result. Other checks store
`verificationUnavailable`; generic absence cannot verify them. Positive-only
sources cannot verify absence, and missing later evidence remains `watching`.
An exact copied prompt remains outside verification until its opaque `ABR-`
reference appears in a later captured user message. The marker publication sets
the boundary and cannot prove the same attempt.

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
| Cline                              | The characterized messages-contract-v1 bundle supports bounded delegated and model findings only. Calls and model names do not prove paired timing, historical resources, or other clean facts; legacy sources fail closed.                                         |
| Kiro canonical and chat            | Separate source shapes; resource definitions, exposure, calls, models, timing, and settings lack characterized detector-grade semantics. The fallback does not inherit canonical coverage.                                                                          |
| Amp thread                         | The characterized thread JSON supports bounded depth and model findings only. Saved routing modes do not prove actual model effort or speed; resources, timing, and accounting remain uncharacterized.                                                              |
| Amp file changes                   | Not a conversation session. No check can use file-change records as request, model, or inventory proof.                                                                                                                                                             |
| Windsurf workspace and mirror JSON | Calls and model names can exist, but complete resource, timing, control, and accounting semantics remain uncharacterized.                                                                                                                                           |
| Windsurf protobuf                  | Discovery recognizes Cascade paths; no bounded protobuf session parser or supported field contract exists.                                                                                                                                                          |
| Generic fallback                   | No native source contract. Recognized-looking JSON does not authorize detector-grade evidence or clean.                                                                                                                                                             |

## Coverage Promotion Rule

Change an entry to `Assessable` only when it can support both findings and clean
results. It needs all of these conditions:

- The accepted source shape is explicit through a schema, header, or pinned
  producer commit and synthetic fixtures. Record a release range when known.
- The reader emits every fact required for both a finding and a clean result.
- Missing, malformed, truncated, capped, or unknown records produce partial or unavailable evidence.
- Positive, negative, and incomplete synthetic fixtures exist.
- Full and resumed reads produce equivalent evidence where resume is supported.
- The model and provider policy is reviewed where the check needs policy.
- The implementation does not use current configuration as historical session evidence.

If a source cannot meet these conditions, keep the supported scope as `Partial`,
`Unsupported`, or `Unknown`. Do not convert missing evidence into a clean
result. Record the source limit and any separate source used for an advisory
assessment. The confirmation ledger records current reviewed decisions.

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
