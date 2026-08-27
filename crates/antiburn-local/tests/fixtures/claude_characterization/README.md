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
| `unrecognized_type.jsonl` | A `telemetry_ping` record without a role or message produces no event. A record with the same type and a recognized assistant role produces one event. Valid neighbours survive both records. |
| `housekeeping_records.jsonl` | The thirteen recognized housekeeping record types (`permission-mode`, `mode`, `last-prompt`, `ai-title`, `queue-operation`, `file-history-delta`, `pr-link`, `atis-latch`, `worktree-state`, `relocated`, `frame-link`, `cost-state`, `agent-name`) produce no events. The session keeps `Complete` coverage and records no unrecognized discriminators. |
| `parent_with_task_spawn.jsonl` | A parent transcript records a `Task` tool spawn without adding child events to the parent. |
| `subagent_child.jsonl` | The child source has its own session ID and metrics. An analysis call over both files does not count either source twice. |
| `multi_model_session.jsonl` | Assistant turns across three synthetic models attribute tokens to each model identity and aggregate repeated turns of the same model. |
| `compaction_with_cache_rehydration.jsonl` | An explicit `compact_boundary` system record marks a compaction and its bucket reports zero context tokens. The cache-creation spike on the turn after the boundary records a rehydration. |
| `inferred_cache_rehydration.jsonl` | Without cache-write tokens, a cache-read collapse on a retained context followed by a same-model recovery turn infers a rehydration. |
| `mcp_and_skill_sources.jsonl` | Skill loading records retain bounded names and one-line self-descriptions; MCP loading records retain names only (multi-line instruction blocks are never persisted). Both match observed invocations and leave origin and built-in definitions unsupported. |
| `reasoning_and_fast_mode.jsonl` | Explicit effort and speed fields split main-loop turns from delegated turns without reading prompt text. |
| `delegated_turns.jsonl` | Sidechain flags identify delegated turns once and preserve their explicit token quantities. |
| `thread_identity_chain.jsonl` | Every record carries a `uuid`, every non-root `parentUuid` resolves in-file (including a sidechain rooted at `parentUuid: null`), and the cache group's `previous_turn` verifies as `Complete`. The model switch next to paid cache writes gives Cache Churn a real finding. |
| `thread_identity_missing_uuid.jsonl` | One counted turn carries no `uuid`, so `previous_turn` and the cache group degrade to `Partial` with `attribution_incomplete` and Cache Churn cannot read clean. |

The checked-in goldens serialize `NormalizedSession` and the complete `SessionMetrics` value for each fixture. The tests compare parsed JSON values, so a field addition fails until a reviewer accepts the golden change.

The large-source tests generate their JSONL in memory. They do not commit a large transcript-shaped blob.

## Claude capability and coverage matrix

An empty supported collection means that the session had no matching record. `Unsupported` means that the Claude format represented by these fixtures cannot state the fact.

| Evidence group | Capability flags | Claude state | Unsupported fact, reason, and upgrade condition | Proving fixture |
| --- | --- | --- | --- | --- |
| `time_range` | `timestamps_and_order=true` | `Complete` or record-loss `Partial` | None | `timestamps_repeated_and_out_of_order.jsonl` |
| `eligibility` | No separate source flag | `Complete` or record-loss `Partial` | None | `records_all_kinds.jsonl` |
| `context` | `request_context_tokens=true` | `Complete` or bounded/record-loss `Partial` | None | `records_all_kinds.jsonl` |
| `models` | `model_identity=true`; `token_classes=true`; `reasoning_effort_tier=true`; `fast_tier=true`; `service_tier=false` | `Complete`, attribution/cap/record-loss `Partial` | `service_tiers` is unsupported because no fixture carries an explicit service tier. Upgrade after an explicit service-tier record is captured as a synthetic fixture. | `multi_model_session.jsonl`; `reasoning_and_fast_mode.jsonl` |
| `tools` | `tool_invocations=true` | `Complete` or bounded/record-loss `Partial` | An unmatched invocation is `Unclassified`, not a built-in definition. | `mcp_and_skill_sources.jsonl` |
| `context_sources` | `skill_mcp_attribution=true`; `tool_definitions=false` | `Complete` or bounded/record-loss `Partial` | `tool_definitions` is unsupported because no fixture carries provider/version built-in definitions. Source `origin` is unsupported because loading records name no origin. Upgrade each fact after its explicit record field is fixtured. | `mcp_and_skill_sources.jsonl` |
| `subagents` | `subagent_relationships=true`; `subagent_models=false` | `Complete` or bounded/record-loss `Partial` | `child_model` is unsupported because a `Task` spawn carries no delegated model. Upgrade after a spawn carries a model or a later seam joins a child transcript. | `parent_with_task_spawn.jsonl`; `delegated_turns.jsonl` |
| `cache` | `cache_write_tokens=true`; `thread_identity=true` | `Complete` or bounded/record-loss `Partial` | `previous_turn` is now evidenced from per-record `uuid` / `parentUuid`: it is `Complete` when every counted turn carries a `uuid` and every non-root `parentUuid` resolves to an identity declared earlier in the same source, and it degrades — together with the cache group — to `Partial` (`attribution_incomplete`) when a counted turn lacks a `uuid` or a parent link does not resolve in-file (for example a resumed session pointing into another file). `provider_eviction` is unsupported because no record states an eviction. Upgrade it after an explicit record shape is fixtured. | `thread_identity_chain.jsonl`; `thread_identity_missing_uuid.jsonl` |
| `compactions` | `compaction_boundaries=true` | `Complete` or bounded/record-loss `Partial` | None | `compaction_with_cache_rehydration.jsonl` |
| `quota_incidents` | `quota_incidents=false` | `Unsupported` | No fixture carries a rate-limit or quota incident. Upgrade after an explicit incident record is captured as a synthetic fixture. | None |

`provenance.harness_version` uses `harness_version=false` and stays `Unsupported`. No fixture carries a harness version discriminator. Upgrade after an explicit version field is fixtured.
