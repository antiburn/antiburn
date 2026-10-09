# Synthetic Pi characterization fixtures

These fixtures are hand-authored synthetic Pi records. They come from
the pinned Pi session contract, its V1/V2 migrations, and the reviewed example
extension. They use only invented values and contain no captured session data.

An accepted source starts with one `type: "session"` record and a valid
timestamp. The reader accepts the official V1 form with no `version`, V2, and
V3. It applies Pi's documented V1 linear-ID and V2 `hookMessage` migrations
in memory. It rejects headerless, malformed, unsupported-version, and
duplicate-ID input before it can emit Pi evidence. The `headerless_*` fixtures
exist only to prove that rejection.

The adapter treats the top-level timestamp as authoritative. It accounts for
only the four disjoint usage buckets. Optional scope content reads the pinned
`plan-mode` and `plan-mode-execute` payload shapes below. Diagnostics can store
only bounded native row, role, and
content-block discriminators when a structural check fails closed. Persisted
evidence and metrics do not retain transcript content, paths, or extension
payload values. They retain bounded session identity and provider/API/model
facts when those facts are required for evidence.

## Core retained V3 contract

`core_linear_v3.jsonl` is independently authored synthetic data. Its shape follows
the inspected Pi 0.84.4 native capture and producer commit
`b79e4cc834970cca69daebffab7df1da7d1e52c4` (`core/agent-session.ts`,
`core/tools/read.ts`, and `core/tools/truncate.ts`). It contains no capture text,
machine paths, or native identities. This is a shape pin, not a released range.

The first entry can be `model_change`. Every subsequent entry must name the
immediately preceding retained entry. Missing IDs, missing parents, duplicate
IDs, multiple roots, siblings, forward references, explicit loss, fork headers,
compaction, and unsupported context-changing entries block retained-root scope.
No extension or branch recovery is required. This proves only retained linear
ancestry; it cannot detect arbitrary external deletion and relinking.

The retained-scope contract requires explicit `SourceFormat::PiV3Jsonl` input.
Legacy metric callers with `Uncharacterized` input retain their existing metric,
cache, fork-ownership, and lineage behavior. They receive no native history,
selected-skill, or observed-read proof. A missing scope contract does not add a
metric attribution gap. The scoped path keeps its stricter admission checks.

Results require one unique earlier tool call, the exact `toolCallId` and tool
name, and explicit `isError`. The status describes the recorded tool result,
not semantic test coverage or user approval. Missing status and unmatched results
stay unknown. Result text and earlier input bindings retain their native record
IDs. Bash clipping prevents complete-result use. Calls are bounded to 4,096 and
8 MiB of identity/input state; exceeded bounds block retained-root scope.

Native `read` results retain exact UTF-8 file-text ranges separately from
continuation notices. Returned line counts come from observed text, not the
requested limit. The contract accepts positive integer offsets/limits, plain
text, exact user-limit notices, and matching byte/line clipping details. A final
newline retains an empty file line. Image results, oversized-first-line notices,
invalid details, and unavailable text do not establish returned extent. A slice
with a continuation notice is not whole-file access. Read metadata requires the
shared normalizer to preserve adapter facts and assign action references.

Core selected skill wrappers remain unknown-authority document context. Only
the exact trailing argument range is separate user text. Malformed or nested
wrapper-like text stays unknown. The journal does not prove human origin,
extension absence, trusted skill provenance, or successful skill execution.
Complete text-only user ranges carry `UserTextHistoryProof` for Pi V3 and the
header session ID, with exact native entry/range bindings. Wrapper documents
do not carry that proof. Mixed image/text user messages block retained scope.
Shared history validation must check the entire source's final coverage, native
session identity, ranges, and skill-document exclusions before admission. Typed
skill proof and production admission remain shared integration requirements;
these parser fixtures alone do not establish desktop support.

## Optional question and plan scope

`scope_records.jsonl` and `pi_scope_characterization.rs` use synthetic records
based on these producer pins:

- Pi coding-agent `0.85.1`, pi-mono commit
  `b2602be77cb7b0de45dd616407fd210daa48aa75`: official
  `examples/extensions/question.ts`, `questionnaire.ts`, and `plan-mode/`.
- `pi-ask-user` `0.16.0`, edlsh/pi-ask-user commit
  `adcc7b2ee22bb290f9d6693efe6c0d0cd1273280`: `index.ts`.
- The Pi pin's `src/core/agent-session.ts` persists `toolResult` messages with
  `SessionManager.appendMessage`. `src/core/session-manager.ts` writes the
  message, including `details`, without removing extension fields. It also
  writes custom state and custom messages with `id` and `parentId`.

The adapter joins calls and results only through the same session's recorded
ancestry. It preserves full call option descriptions. Multiple questionnaire
questions are single-select questions. Questionnaire indices are one-based in
native records and zero-based in normalized selections. Cancelled partial
questionnaires retain their recorded selections without submission authority.
Noninteractive error results with empty questions remain unknown. Structured
cancellation requires a recorded question that matches the call.
Question text-only results require the exact pinned wording. Questionnaire
text-only results support unique question labels and unambiguous single-line
batch answers; a single custom answer can contain newlines.

`ask_user` supports single and batch details, selections, comments, freeform
answers, and skipped questions. A timeout and cancellation have the same
persisted result shape. The adapter does not infer a timed-out status. Typed
records retain context, comments, questionnaire display labels as `header`, and
skipped batch status. Native string bindings retain call and result record IDs.

These pins identify accepted shapes, not the installed extension. Pi's journal
does not persist a trusted producer identity for these tools. All question
origins and provenance remain unknown, including records with matching names
and shapes. Ordinary extension output does not become user authorization.

Plan state and todos alone do not establish acceptance. A synthetic execution
choice requires the exact state transition, todo-list message, and execution
message on the same ancestry. Full plan text is bound only when the persisted
todos match the recorded assistant proposal. Todo cleaning uses the producer's
UTF-16 length and truncation rules; a split surrogate stays unresolved. The full
proposal retains its original UTF-8. Truncation or incompleteness on the proposal,
state, todo-list message, execution message, or other ancestry keeps the plan
unresolved, with no content digest or approved version binding. Resume snapshots
retain these completeness flags. Todo cleaning does not restore missing text.
No mutable external plan file is read. Branched journals retain
the reader's incomplete attribution status; the adapter does not select a leaf.

The tests cover cancellation, noninteractive errors, unknown names, spoofed
execution text, missing calls, sibling ancestry, partial answers, and unavailable
proposal versions. These are shape contracts with synthetic fixtures, not a
claim about all Pi or extension releases. Scope lineage is bounded to 50,000
rows and 8 MiB of retained state; an exceeded bound marks attribution incomplete.
Ancestry traversal also has a 50,000-row bound and detects repeated IDs, missing
parents, and self-links. It rejects invalid paths without partial joins and
checks cancellation on each traversal step, including restored snapshots.

Pi supports request occupancy, cache writes when the selected API reports
them, timestamps, tool calls, model identity, token classes, thinking levels,
compaction boundaries, record identity, and thread identity. Cache findings
also require a recovered miss episode on one reviewed route. It does not claim
tool catalogs, MCP attribution, speed or service tiers, quota events, or a
harness version. The reviewed example extension can provide finding-only
subagent evidence. Arbitrary extensions remain fail closed.

Every entry after the `session` header carries a top-level `id` and
`parentId`. Exactly one entry per file has `parentId: null` — the thread
root. Every fixture below gives its rows a realistic `id` / `parentId` chain
unless the fixture's own purpose is to be malformed, in which case the
missing or broken chain is the point.

- `session_overdepth_finding.jsonl` reports one turn's input tokens above the Sessions Over Depth cap, giving that badge a finding.
- `model_overthinking_finding.jsonl` sets `thinkingLevel` to `max`, giving Model Overthinking a finding.
- `excess_cache_rehydration_finding.jsonl` preserves an older model-switch and cache-write shape; it does not establish a current cache-rehydration finding.
- `cache_continuous_transient_miss.jsonl` records a same-route hit, miss, and recovery during continuous activity; accounting remains visible without a rehydration finding.
- `overthinking_aborted_zero_usage_content.jsonl` carries content and an above-cap effort setting on an aborted message with zero usage; it must not establish an effort finding.
