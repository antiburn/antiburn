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
| `within_file_duplicate_uuid.jsonl` | An assistant record is re-logged with the same `uuid`, `parentUuid`, `timestamp`, and `message.id` as an earlier copy (the same re-logging Claude Code does mid-stream). The chain stays one thread, the existing `message.id` usage de-duplication keeps its tokens from double counting, and the duplicate-identity diagnostic — a cross-source-key signal — stays at zero. |
| `session_overdepth_finding.jsonl` | One main-loop turn reports input tokens above the Sessions Over Depth cap, giving that badge a finding. |
| `model_overthinking_finding.jsonl` | One main-loop turn reports explicit effort `max`, giving Model Overthinking a finding. |
| `fast_mode_overuse_clean.jsonl` | A main-loop and a delegated turn both report explicit speed `standard`, so Fast-Mode Overuse reads clean instead of not-assessed. |
| `fork_replay_parent.jsonl`, `fork_replay_subagent.jsonl` + `.meta.json`, `fork_replay_fork.jsonl` + `.meta.json` | A fork sub-agent's `.meta.json` sidecar carries `isFork` and `parentAgentId`; its transcript replays its direct parent's records under the same `uuid` before it appends its own new record. `tests/claude_characterization.rs`'s `fork_replay_session` writes these three transcripts (plus the fork's sidecar) into a real `subagents/` directory so the ingest path can resolve the replay source from the file layout, the same way it does against a real Claude Code session. |

The following policy fixtures carry no metrics golden. `tests/unrecognized_records.rs` exercises them directly.

| Policy fixture | Proves |
| --- | --- |
| `unrecognized_role_with_usage.jsonl` | An unknown role with usage fails closed, while valid neighbours survive. |
| `unrecognized_evidence_shapes.jsonl` | Tool, thinking, compaction, model, usage, split function-call, and allowlisted evidence shapes fail closed. |
| `unrecognized_inert_records.jsonl` | Several inert unknown types keep complete coverage and enter a detector denominator beside real assistant work. |
| `unrecognized_inert_sidechain.jsonl` | Sidechain evidence is observed before inert classification, so downstream contract-incomplete status remains reachable. |

The checked-in goldens serialize `NormalizedSession` and the complete `SessionMetrics` value for each golden fixture. The tests compare parsed JSON values, so a field addition fails until a reviewer accepts the golden change.

The large-source tests generate their JSONL in memory. They do not commit a large transcript-shaped blob.

## Claude capability and coverage matrix

An empty supported collection means that the session had no matching record. `Unsupported` means that the Claude format represented by these fixtures cannot state the fact. Unknown variants degrade supported groups only when they are evidence-bearing or their discriminator bounds are exceeded. Structurally inert unknowns keep complete coverage.

| Evidence group | Capability flags | Claude state | Unsupported fact, reason, and upgrade condition | Proving fixture |
| --- | --- | --- | --- | --- |
| `time_range` | `timestamps_and_order=true` | `Complete` or record-loss `Partial` | None | `timestamps_repeated_and_out_of_order.jsonl` |
| `eligibility` | No separate source flag | `Complete` or record-loss `Partial` | None | `records_all_kinds.jsonl` |
| `context` | `request_context_tokens=true` | `Complete` or bounded/record-loss `Partial` | None | `records_all_kinds.jsonl` |
| `models` | `model_identity=true`; `token_classes=true`; `reasoning_effort_tier=true`; `fast_tier=true`; `service_tier=false` | `Complete`, attribution/cap/record-loss `Partial` | `service_tiers` is unsupported because no fixture carries an explicit service tier. Upgrade after an explicit service-tier record is captured as a synthetic fixture. | `multi_model_session.jsonl`; `reasoning_and_fast_mode.jsonl` |
| `tools` | `tool_invocations=true` | `Complete` or bounded/record-loss `Partial` | An unmatched invocation is `Unclassified`, not a built-in definition. | `mcp_and_skill_sources.jsonl` |
| `context_sources` | `skill_mcp_attribution=true`; `tool_definitions=false` | `Complete` or bounded/record-loss `Partial` | `tool_definitions` is infeasible from the characterized transcript shape. A source-fixture key census finds no `tools`, `toolDefinitions`, or `availableTools` catalogue key. A provider/version catalogue belongs in the harness knowledge base. Source `origin` is unsupported because loading records name no origin. | `mcp_and_skill_sources.jsonl` |
| `subagents` | `subagent_relationships=true`; `subagent_models=true` | `Complete`, attribution/cap/record-loss `Partial` | `delegated_models` is a bounded session-level set from sidechain assistant `message.model`. A missing delegated model degrades the group to attribution-incomplete `Partial`. `child_model` stays unsupported because a sidechain root does not identify its `Task` spawn. Upgrade the child field only after a verified spawn edge exists. | `parent_with_task_spawn.jsonl`; `delegated_models.jsonl`; `delegated_model_missing.jsonl` |
| `cache` | `cache_write_tokens=true`; `record_identity=true` | `Complete` or bounded/record-loss `Partial` | `previous_turn` is now evidenced from per-record `uuid` / `parentUuid`, falling back to `logicalParentUuid` when `parentUuid` is null (a compaction boundary): it is `Complete` when every counted turn carries a `uuid` and every non-root `parentUuid` (or its `logicalParentUuid` fallback) resolves to an identity declared earlier in the same source, and it degrades — together with the cache group — to `Partial` (`attribution_incomplete`) when a counted turn lacks a `uuid` or a parent link does not resolve in-file (for example a resumed session pointing into another file). `provider_eviction` is unsupported because no record states an eviction. Upgrade it after an explicit record shape is fixtured. | `thread_identity_chain.jsonl`; `thread_identity_missing_uuid.jsonl`; `compaction_continues_thread.jsonl` |
| `compactions` | `compaction_boundaries=true` | `Complete` or bounded/record-loss `Partial` | None | `compaction_with_cache_rehydration.jsonl` |
| `quota_incidents` | `quota_incidents=false` | `Unsupported` | No fixture carries a rate-limit or quota incident. Upgrade after an explicit incident record is captured as a synthetic fixture. | None |

`provenance.harness_version` uses `harness_version=false` and stays `Unsupported`. No fixture carries a harness version discriminator. Upgrade after an explicit version field is fixtured.
