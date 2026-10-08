# Claude characterization fixtures

These fixtures freeze the Claude JSONL shapes that the first Local Insights slice supports. The integration test reads them through the public normalization and analysis APIs.

**Every file here is synthetic.** Each file was authored by hand from format knowledge and the current parser. No real session, user, machine, organization, or repository is represented. The fictional user is `avery`. The fictional project is `/home/avery/projects/demo-app`. The `orbit-tracker` and `atlas-notes` skills are invented.

The parser and these in-repository fixtures define the supported Claude record shapes. Repeatable checks against public CLI versions can inform future fixture work.

No captured provider session log ever enters this repository. Do not copy one. Do not redact one into a fixture. Redaction is not sufficient.

| File | Proves |
| --- | --- |
| `records_all_kinds.jsonl` | User and assistant records preserve usage, model, tool use, error tool results, thinking, and compaction. The fixture also provides positive initial-context and skill-description signals. |
| `timestamps_repeated_and_out_of_order.jsonl` | Repeated and non-monotonic timestamps produce stable duration, active time, and buckets. |
| `malformed_between_valid.jsonl` | The parser skips one malformed line and keeps valid records on both sides. CH-004 must also assert `Partial` coverage because the current API exposes no coverage value. |
| `incomplete_final_record.jsonl` | The parser does not commit the truncated final record. The next source generation can pick up the record after its terminating newline arrives. |
| `unrecognized_type.jsonl` | A structurally inert `telemetry_ping` record keeps `Complete` coverage and retains its discriminator. A record with the same type and a recognized assistant role produces one event. Valid neighbours survive both records. |
| `housekeeping_records.jsonl` | The fifteen recognized housekeeping record types (`permission-mode`, `mode`, `last-prompt`, `ai-title`, `queue-operation`, `file-history-delta`, `pr-link`, `atis-latch`, `worktree-state`, `relocated`, `frame-link`, `cost-state`, `agent-name`, `history-suppression`, `artifact-autoreact-ledger`) produce no events. The session keeps `Complete` coverage and records no unrecognized discriminators. |
| `parent_with_task_spawn.jsonl` | A parent transcript records a `Task` tool spawn without adding child events to the parent. |
| `subagent_child.jsonl` | The child source has its own session ID and metrics. An analysis call over both files does not count either source twice. |
| `multi_model_session.jsonl` | Assistant turns across three synthetic models attribute tokens to each model identity and aggregate repeated turns of the same model. |
| `compaction_with_cache_rehydration.jsonl` | An explicit `compact_boundary` system record marks a compaction and its bucket reports zero context tokens. The cache-creation spike on the turn after the boundary records a rehydration. |
| `inferred_cache_rehydration.jsonl` | Without cache-write tokens, a cache-read collapse on a retained context followed by a same-model recovery turn infers a rehydration. |
| `mcp_and_skill_sources.jsonl` | Skill loading records retain bounded names and one-line self-descriptions; MCP loading records retain names only (multi-line instruction blocks are never persisted). Both match observed invocations and leave origin and built-in definitions unsupported. |
| `reasoning_and_fast_mode.jsonl` | Explicit effort and speed fields split main-loop turns from delegated turns without reading prompt text. |
| `delegated_turns.jsonl` | Sidechain flags identify delegated turns once and preserve their explicit token quantities. |
| `delegated_models.jsonl` | Sidechain assistant records retain their explicit `message.model` values as a bounded session-level set. A premium parent and delegated model give Overpowered Subagents a finding. |
| `delegated_model_missing.jsonl` | A sidechain assistant record without `message.model` degrades the subagent group to attribution-incomplete `Partial`, so Overpowered Subagents cannot read clean. |
| `thread_identity_chain.jsonl` | Every record carries a `uuid`, every non-root `parentUuid` resolves in-file (including a sidechain rooted at `parentUuid: null`), and the cache group's `previous_turn` verifies as `Complete`. The model switch next to paid cache writes gives Cache Churn a real finding. |
| `thread_identity_missing_uuid.jsonl` | One counted turn carries no `uuid`, so `previous_turn` and the cache group degrade to `Partial` with `attribution_incomplete` and Cache Churn cannot read clean. |
| `sidechain_in_parent.jsonl` | A sidechain record inside the parent transcript keeps subagent token classification during bounded merge. |
| `late_skill_metrics.jsonl` | A slash-command skill resolved at finish keeps its original position and duration. |
| `two_compactions_second_without_metadata.jsonl` | Two same-position compactions keep the last boundary's empty metadata as one tuple. |
| `rehydration_gap_none.jsonl` | An inferred cache miss without its own timestamp preserves the unknown-gap rehydration rule. |
| `disorder_ladder.jsonl` | A turn displaced by 64 arrivals exercises the reorder-window overflow and timestamp clamp. |
| `subagent_single_timestamp.jsonl` | A delegated stream with one repeated timestamp keeps its ordinal order before merged placement. |
| `compaction_continues_thread.jsonl` | A `compact_boundary` record's `parentUuid` is null, but its `logicalParentUuid` names the last pre-compaction record, so the main loop stays one thread across it. The model switch on either side of the boundary still counts as a transition, and the boundary itself is one manual compaction. |
| `inline_sidechain_own_thread.jsonl` | Four `isSidechain: true` records inline in the parent transcript, rooted at `parentUuid: null` on a different model, get their own thread. The main loop's own transitions and idle gaps never see the sidechain's turns or model. |
| `within_file_duplicate_uuid.jsonl` | A synthetic guard, not an observed Claude Code behavior: an assistant record is repeated under the same `uuid`, `parentUuid`, `timestamp`, and `message.id` as its first copy, adjacent to it in the same file. `seen_uuids` treats the repeat as an in-file replay exactly like a distant resume replay and skips it (`recordsReplayed` 1), so only the first copy's usage counts. The chain stays one thread and the duplicate-identity diagnostic — a cross-source-key signal — stays at zero. |
| `session_overdepth_finding.jsonl` | One main-loop turn reports input tokens above the Sessions Over Depth cap, giving that badge a finding. |
| `model_overthinking_finding.jsonl` | One main-loop turn reports explicit effort `max`, giving Model Overthinking a finding. |
| `fast_mode_overuse_clean.jsonl` | A main-loop and a delegated turn both report explicit speed `standard`, so Fast-Mode Overuse reads clean instead of not-assessed. |
| `fork_replay_parent.jsonl`, `fork_replay_subagent.jsonl` + `.meta.json`, `fork_replay_fork.jsonl` + `.meta.json` | A fork sub-agent's `.meta.json` sidecar carries `isFork` and `parentAgentId`; its transcript replays its direct parent's records under the same `uuid` before it appends its own new record. `tests/claude_characterization.rs`'s `fork_replay_session` writes these three transcripts (plus the fork's sidecar) into a real `subagents/` directory so the ingest path can resolve the replay source from the file layout, the same way it does against a real Claude Code session. |
| `api_error_records.jsonl` | An `isApiErrorMessage` assistant record's `apiErrorStatus` and `error` fields map to a quota or provider incident, reviewed against harness version `2.1.270`: `529` is `Capacity`, another `5xx` status or `error: "server_error"` with no usable status is `ServerError`, and `429` or `error: "rate_limit"` with no status is a `QuotaIncident`. `error: "unknown"` with no status, a non-429 4xx, a record without `isApiErrorMessage`, and a record with no top-level `timestamp` all produce nothing. Every mapped incident's model comes from the last real request's `message.model`, never from the error record's own `"<synthetic>"` model. |
| `cowork_transcript.jsonl` | A Claude Desktop Cowork transcript from a nested `<app-config>/Claude/local-agent-mode-sessions/**/.claude/projects/<slug>/` root parses its `message.usage` token counts like any `ClaudeJsonl` source. The extra `server_tool_use` and `service_tier` usage keys and the Cowork top-level keys keep `Complete` coverage. The record keys follow a key-only observation of Claude Desktop 2.2553.1 with embedded Claude Code 2.1.275; there is no `entrypoint` field. It carries no golden; `a_claude_desktop_cowork_transcript_parses_with_usage` asserts the token totals. |
| `cowork_audit.jsonl` | The Cowork `audit.jsonl` shape beside the nested `.claude` directory. Its `assistant` records repeat the transcript's `message.usage`, so it is not a token ledger. `a_cowork_audit_log_is_not_discovered_beside_its_transcript` proves that discovery returns only the transcript. |

The following policy fixtures carry no metrics golden. `tests/unrecognized_records.rs` exercises them directly.

| Policy fixture | Proves |
| --- | --- |
| `unrecognized_role_with_usage.jsonl` | An unknown role with usage fails closed, while valid neighbours survive. |
| `unrecognized_evidence_shapes.jsonl` | Tool, thinking, compaction, model, usage, split function-call, and allowlisted evidence shapes fail closed. |
| `unrecognized_inert_records.jsonl` | Several inert unknown types keep complete coverage and enter a detector denominator beside real assistant work. |
| `unrecognized_inert_sidechain.jsonl` | Sidechain evidence is observed before inert classification, so downstream contract-incomplete status remains reachable. |

The checked-in goldens serialize `NormalizedSession` and the complete `SessionMetrics` value for each golden fixture. The tests compare parsed JSON values, so a field addition fails until a reviewer accepts the golden change.

The large-source tests generate their JSONL in memory. They do not commit a large transcript-shaped blob.

## Optional question and plan contracts

`scope_records.jsonl` is synthetic native JSONL. `support/claude_scope.rs` tests
it through the Claude reader. It retains question prompts, full descriptions,
single exact answer strings, multi-select flags, and exact plan text versions.
Negative controls cover missing/malformed answers, interruption, synthetic
records, conflicting calls/questions, ambiguous result envelopes, incomplete
reads, mismatched writes, rejection followed by ordinary user free text, and
automatic plan permission. None of these records proves human scope approval.

Scope result joins also require a complete recorded native parent chain. Sharing
`event.thread_id` proves only a common root, not descent from the tool call.
The parser checks `uuid` and `parentUuid`, with explicit `logicalParentUuid`
fallback at a null primary parent, before it consumes pending call metadata.
Same-root sibling results and their descendants cannot consume question,
ExitPlanMode, Read, or Write calls. Missing identities, missing/unresolved links,
conflicting record identities, and cycles do not resolve results. True
descendants can still resolve the pending call after a rejected sibling result.
The native ancestry map persists in the Claude adapter snapshot. Characterization
tests compare full reads with resumed reads after the call, sibling results, and
an intermediate descendant. The map retains at most 16,384 records and 4 MiB of
identity text, with 512-byte identities and a 4,096-record traversal limit.
Limit loss does not fall back to thread-root or arrival-order matching.

Accepted public contract pins:

- `futpib/claudex@0ad5073179efbfcc9dd9d6a9c19cca4575431653`,
  `src/transcript/parser.ts` and `parser.test.ts`: assistant tool-use ID/name/input,
  user result ID/content, and top-level string-valued `toolUseResult.answers`.
  Its answer-only fixture has empty input; options need the separate input shape.
- `TrafficGuard/typedai@34139aec65bb70f7062cf7f92667c11ffde4fcb1`,
  `.claude/hooks/extract-qa.py`: native `input.questions`, labels/descriptions,
  and question-text answer mapping. Recorded `multiSelect` preserves its boolean;
  SDK callback return values do not define CLI persistence.
- Published `vct-core 2.7.1`, archive SHA-256
  `fc41d67b80db72fc9e13cd6a71e76846cc20a399fb8c9d3592849a42f58c2f82`,
  `src/session/claude.rs`: `update_write_results_recover_details_without_double_counting`
  and `exit_plan_mode_file_path_is_not_a_file_operation`. The archive's VCS
  metadata reports dirty state, so its Git SHA is not a substitute for this pin.
- `folke/zaly@5a113518e6b0790fa0a63d058c3b5e76f8858e55`,
  `packages/agent/test/claude.test.ts`: Read result `file.content`, path,
  `startLine`, `numLines`, and `totalLines`.
- `anthropics/claude-code#81223`, comments `5080551532` and `5127759342`:
  rejection/interruption ambiguity and later ordinary single-select free text.
- `anthropics/claude-code#24302`, updated `2026-03-19T14:31:09Z`, reported
  CLI 2.1.37: `planContent` user-record text can be synthetic and stale.

These are decoder/accepted-log contracts, not a public Claude CLI producer pin
or a historical release range. The old cclens `8246ffa3` source is unavailable
(URL 404; commit API 422) and is not evidence. Structured answers have unknown
origin. Plan completion, permission changes, and approval wording never produce
approved revisions. Exact recorded text has SHA-256 identity; mutable references
remain unresolved. No companion file is read. Custom plan directories and Edit
reconstruction are outside this accepted subset.

## Retained CLI root and native results

`retained_native_results.jsonl` is synthetic. Its envelopes, origin markers,
Read payload, Bash payload, and failed Skill result follow local CLI 2.1.278
records inspected on 2026-10-07. The local root Read records were in the
`antiburn-cloud` project, not the `antiburn` project named in the research lead.
No private transcript text, paths, IDs, or command output is copied here.
The private producer has no public schema pin. This is an accepted-log shape,
not a supported historical release range.

The root contract requires `version:2.1.278`, `entrypoint:cli`, a bounded
`sessionId`, `isSidechain:false`, no `agentId`, and an explicit null root parent.
Every subsequent identity must link directly to the previous retained identity.
The root can be human text or the observed `system/informational` prelude with
`level:notice`, `isMeta:false`, and string `content`. Human messages must carry
`origin.kind:human`, `promptSource:typed`, and `turnOrigin:human`. Human text-only
messages receive `UserTextHistoryProof` revision 1 and exact `UserMessage`
bindings to `/message/content` or `/message/content/{index}/text`. Ranges use
decoded UTF-8 byte offsets and the containing record UUID. No subset proof is
issued when a text block is missing, mismatched, or clipped.
The adapter retains at most 16,384 record fingerprints, with
512-byte native identities. Exact replays do not add scope. Conflicting replays,
missing parents, branches, compaction, malformed records, and fork/child inputs
deny later history proofs. A characterized root with detected loss reports
`AttributionIncomplete`; consumers must reject the entire publication, including
earlier per-message proofs. The proofs alone are not a session-wide admission gate.

The human markers above are present together in the local 2.1.278 CLI capture.
Its first human message has a non-null parent that resolves to an earlier
informational system record. The original human-only null-parent requirement
rejected this real prefix. `retained_informational_root.jsonl` characterizes the
corrected prelude without copying private text or identities. Later ambiguous
links still reject the source; this correction does not admit arbitrary system,
meta, or SDK roots. Local SDK roots have `promptSource:sdk`, `turnOrigin:sdk`,
and no human-origin marker, so they do not receive human history proof.

Native non-human origins, system prompt sources, meta messages, synthetic
messages, and `planContent` do not become human authority. Unknown origin in the
accepted producer remains unknown. The textual content of a human message does
not determine its origin. Question submission, tool permission, skill selection,
and assistant reports do not establish human approval.

Native Read results require a unique earlier call and a complete same-source
descendant chain. Optional `sourceToolUseID` and `sourceToolAssistantUUID` must
match. The accepted text payload uses `filePath`, `content`, `startLine`,
`numLines`, and `totalLines`. Returned extent is recorded only when every tab-
numbered output line matches that payload. It reports observed lines, not the
requested limit or a whole-file read. `is_error` supplies completion/error.
The observed Read producer omits that flag on successful text results. An
exactly matched text payload and numbered output also establish Read completion.
Status stays unknown without either accepted status path. Interruption,
background work, and clipped output also keep status unknown.
Partial-view and persisted-output notices deny returned extent. No external
output file is read. Image/PDF/directory output and alternate renderers have no
accepted text extent. No content digest is presented as a file version.

Bash test output and Skill failure text retain exact result bindings and native
completion/error status. Completion does not mean tests passed. The inspected
root Skill call failed with an unknown-skill string result and `is_error:true`.
This does not prove a successful skill launch or document delivery. Older
conditional launch fixtures keep their separate decoder contract. Injected
skill bodies do not acquire human approval or prove that their instructions ran.

The tests cover successful bounded Read, mismatched extent/path/content, failed
Read, missing status, interruption, output persistence, partial-view notices,
synthetic results, sibling/duplicate/mismatched joins, native human withdrawal,
injected messages, changed producer versions, and full/resumed equivalence.

The claim is about currently retained source records. Documented v2.1.287+
headless/SDK local GC can remove pre-compaction history. No universal native GC
marker is established. This contract does not admit that producer, reconstruct
deleted history, or prove that external deletion never occurred.

## Core capability and coverage matrix

An empty supported collection means that the session had no matching record. `Unsupported` means that the Claude format represented by these fixtures cannot state the fact. Unknown variants degrade supported groups only when they are evidence-bearing or their discriminator bounds are exceeded. Structurally inert unknowns keep complete coverage.

| Evidence group | Capability flags | Claude state | Unsupported fact, reason, and upgrade condition | Proving fixture |
| --- | --- | --- | --- | --- |
| `time_range` | `timestamps_and_order=true` | `Complete` or record-loss `Partial` | None | `timestamps_repeated_and_out_of_order.jsonl` |
| `eligibility` | No separate source flag | `Complete` or record-loss `Partial` | None | `records_all_kinds.jsonl` |
| `context` | `request_context_tokens=true` | `Complete` or bounded/record-loss `Partial` | None | `records_all_kinds.jsonl` |
| `models` | `model_identity=true`; `token_classes=true`; `reasoning_effort_tier=true`; `fast_tier=true`; `service_tier=false` | `Complete`, attribution/cap/record-loss `Partial` | `service_tiers` is unsupported because no fixture carries an explicit service tier. Upgrade after an explicit service-tier record is captured as a synthetic fixture. | `multi_model_session.jsonl`; `reasoning_and_fast_mode.jsonl` |
| `tools` | `tool_invocations=true` | `Complete` or bounded/record-loss `Partial` | An unmatched invocation is `Unclassified`, not a built-in definition. | `mcp_and_skill_sources.jsonl` |
| `context_sources` | `skill_inventory=true`; `mcp_inventory=true`; `tool_definitions=true` | `Complete` or bounded/record-loss `Partial` | `tool_definitions` resolves against the embedded built-in tool catalogue, keyed by the source's harness version and resolved model. No characterization fixture carries a top-level `version` field, so `context_sources.tool_definitions` itself stays `Unsupported` here. Upgrade a fixture to carry a `version` field once a synthetic version is characterized. Source `origin` is unsupported when loading records name no origin. | `mcp_and_skill_sources.jsonl` |
| `subagents` | `subagent_relationships=true`; `subagent_models=true` | `Complete`, attribution/cap/record-loss `Partial` | `delegated_models` is a bounded session-level set from sidechain assistant `message.model`. A missing delegated model degrades the group to attribution-incomplete `Partial`. `child_model` stays unsupported because a sidechain root does not identify its `Task` spawn. Upgrade the child field only after a verified spawn edge exists. | `parent_with_task_spawn.jsonl`; `delegated_models.jsonl`; `delegated_model_missing.jsonl` |
| `cache` | `cache_write_tokens=true`; `record_identity=true` | `Complete` or bounded/record-loss `Partial` | `previous_turn` is now evidenced from per-record `uuid` / `parentUuid`, falling back to `logicalParentUuid` when `parentUuid` is null (a compaction boundary): it is `Complete` when every counted turn carries a `uuid` and every non-root `parentUuid` (or its `logicalParentUuid` fallback) resolves to an identity declared earlier in the same source, and it degrades — together with the cache group — to `Partial` (`attribution_incomplete`) when a counted turn lacks a `uuid` or a parent link does not resolve in-file (for example a resumed session pointing into another file). `provider_eviction` is unsupported because no record states an eviction. Upgrade it after an explicit record shape is fixtured. | `thread_identity_chain.jsonl`; `thread_identity_missing_uuid.jsonl`; `compaction_continues_thread.jsonl` |
| `compactions` | `compaction_boundaries=true` | `Complete` or bounded/record-loss `Partial` | None | `compaction_with_cache_rehydration.jsonl` |
| `quota_incidents` | `quota_incidents=true` | `Complete` | None | `api_error_records.jsonl` |
| `provider_incidents` | `provider_incidents=true` | `Complete` | `ProviderIncidentKind::Connection` is never mapped for Claude: its `error: "unknown"` label is too broad to claim a connection failure without reading the message text. Upgrade after a narrower connection-failure field is characterized. | `api_error_records.jsonl` |

`provenance.harness_version` uses `harness_version=false` and stays `Unsupported`. No fixture carries a harness version discriminator. Upgrade after an explicit version field is fixtured.
