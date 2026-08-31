# Codex rollout characterization

These fixtures are synthetic. They follow the public `openai/codex` rollout types at commit `e9a446d` and contain no captured session data.

Codex writes `{timestamp,type,payload}` JSONL under `~/.codex/sessions/YYYY/MM/DD/`. The rollout policy persists session metadata, turn context, selected response items, token counts, and compaction records. The writer opens a rollout and appends lines. The public rollout code exposes no compaction or in-place rewrite path for these JSONL files. Antiburn still uses a full source recheck because repository fixtures cannot prove external writer behavior. `state_5.sqlite` is a separate state store and is not a rollout source.

## Capability matrix

| Capability | State | Extracted source fact |
| --- | --- | --- |
| Request context tokens | yes | `token_count.info.last_token_usage.input_tokens` and cached input |
| Cache-write tokens | no | The public type defaults the field, but the rollout does not reliably emit it |
| Timestamps and order | yes | Every public `RolloutLine` has `timestamp` |
| Tool invocations | yes | Persisted response tool-call variants; unknown and paginated variants degrade coverage |
| Skill and MCP attribution | no | Legacy calls do not reliably identify a server or skill source |
| Tool definitions | no | No persisted complete tool catalogue |
| Model identity | yes | `turn_context.model` |
| Token classes | yes | Input, cached input, output, and reasoning output are distinct |
| Reasoning effort tier | yes | `turn_context.effort`; missing attribution degrades coverage |
| Fast tier | yes | `event_msg`/`thread_settings_applied.thread_settings.service_tier`, normalized to `fast`/`standard` |
| Service tier | no | The adapter folds the setting into `fast_tier`'s speed vocabulary instead of publishing its own `service_tiers` marker |
| Subagent relationships | yes | A `spawn_agent` function call emits `SubagentSpawn`; discovery relates the spawned child rollout |
| Subagent models | yes | The child's own `turn_context.model` reaches `subagents.delegated_models` through its `Delegated`-scope rows |
| Compaction boundaries | yes | Top-level `compacted` and legacy `context_compacted` |
| Thread identity | yes | One rollout is one thread; a discovered child rollout streams with `Delegated` scope, so a child thread never merges into the parent's main-scope facts |
| Record identity | no | Records carry no per-record id (`uuid`) or parent link, so `previous_turn` stays unsupported |
| Quota incidents | no | The evidence sink has no incident ingestion path in this slice |
| Harness version | no | The evidence sink has no version ingestion path in this slice |

Sessions Over Depth, Model Overthinking, Overpowered Subagents, Old Model Usage, and Fast-Mode Overuse have all capability prerequisites. Every other detector remains not assessed. Cache Churn needs record identity, which Codex does not claim, so it stays not assessed even though Codex claims thread identity. Fast-mode overuse needed Subagent relationships in addition to `fast_tier`; now that the adapter publishes the subagent relationship, both it and Overpowered Subagents move into the assessed set.

An unrecognized `(type, payload.type)` combination no longer fails coverage closed by default (#229 parity). `is_inert_codex_record` proves a record structurally inert — no usage, model, effort, service tier, role, tool-shaped, or compaction-shaped keys at the depth its readers cover — before the record is skipped with `Complete` coverage and its discriminator retained. A record that fails the proof stays `Unusable(UnrecognizedRecordType)`, exactly as before. `event_msg`/`item_completed` and top-level `inter_agent_communication_metadata` are allowlisted as proven echoes of records this adapter already models (measured against 1,034 local rollouts: no sampled record of either family carried usage, a model, or an effort) and pass the lighter check that only reads the record's root and root `payload` object; every other unrecognized family is proved inert one record at a time by the strict, any-depth check. `session_meta`, `turn_context`, `world_state`, and the pre-existing `event_msg` housekeeping payloads bypass the structural check entirely: their own evidence-bearing fields (`turn_context.model`/`.effort`, `thread_settings_applied`'s `service_tier`) are read by `observe_model_and_effort` / `service_tier_speed` on every record, before classification runs, so nothing about them is left unproven.

Codex multi-agent ("collab") sessions add a tenth `event_msg` family: `collab_agent_spawn_begin`/`_end`, `collab_agent_interaction_begin`/`_end`, `collab_waiting_begin`/`_end`, `collab_close_begin`/`_end`, and `collab_resume_begin`/`_end`. Each pairs a begin and an end record around one step of an inter-agent call, and each field repeats data the session's own `spawn_agent` function call already carries. A verified read of the public `openai/codex` protocol source's `EventMsg` enum (not a sample) found no usage, token, or billing field on any of the ten payload structs, so all ten are allowlisted the same way `session_meta`/`turn_context` are and bypass the structural check entirely. `collab_agent_spawn_begin`/`_end` do carry `model` and `reasoning_effort`, naming the spawned agent's own configuration rather than billing evidence; those two fields are read by `observe_model_and_effort` before classification runs, same as `turn_context.model`/`.effort`, so nothing about them is left unproven either. The other eight variants carry no evidence-bearing field at all.

## Coverage cases

- `records_all_kinds.jsonl` covers supported legacy records and duplicate compaction markers.
- `malformed_between_valid.jsonl` keeps valid neighbors and reports partial coverage.
- `unrecognized_type.jsonl` carries a single `event_msg`/`item_completed` record with a made-up `item.type`. Since `item_completed` is now allowlisted and structurally inert (the unknown `item.type` sits inside the echoed `item`, past the light check's depth), this now reports `Complete` coverage and records no discriminator; see `inert_unknown_event.jsonl` for a genuinely unrecognized `event_msg` payload type instead.
- `absent_model_and_effort.jsonl` reports incomplete attribution instead of a clean model result.
- `resolved_fork.jsonl` excludes replayed parent token counts and keeps child-owned usage.
- `fork_developer_lookbehind.jsonl` includes the developer row immediately before the owned task boundary.
- `fork_disputed_window.jsonl` keeps usage between the owned task boundary and its child discriminator.
- `unresolved_fork.jsonl` attributes all usage when the child discriminator is absent.
- `incomplete_final_record.jsonl` models an active writer stopped inside its final line.
- `service_tier_priority.jsonl` changes `thread_settings.service_tier` from `default` to `priority` and back, and checks the resulting `fast`/`standard` split.
- `service_tier_absent.jsonl` records no `thread_settings_applied` at all, so `speed_signal` reports zero present turns.
- `spawn_agent.jsonl` has a parent turn call `spawn_agent`, and checks that the call publishes a subagent relationship and keeps the subagent-evidence detectors assessable.
- `collab_agent_records.jsonl` has a parent turn call `spawn_agent`, then the full ten-variant collab family: a `spawn_begin`/`spawn_end` pair in the pre-completion-tracking shape (no `completed_at_ms`, string `status`), a second pair in the current shape (`completed_at_ms` present, object `status`), an `interaction_begin`/`interaction_end` pair, a `waiting_begin`/`waiting_end` pair, and a `close_end`. Coverage stays `Complete`, `records_unusable` and `records_unrecognized_inert` are both `0`, metrics match the same fixture with the collab lines removed, and the `spawn_agent` call still publishes exactly one `SubagentSpawn`.
- `session_overdepth_finding.jsonl` reports one turn's input tokens above the Sessions Over Depth cap, giving that badge a finding.
- `model_overthinking_finding.jsonl` sets `turn_context.effort` to `max`, giving Model Overthinking a finding.

### #229-parity cases (no golden; exercised by dedicated assertions in `codex_characterization.rs`)

- `item_completed_echo.jsonl` has an `event_msg`/`item_completed` record after each of a user message, an assistant message, and a function call (`item.type` `UserMessage` / `AgentMessage` / `CommandExecution`, the last with `command`, `exit_code`, `stdout`, `aggregated_output` keys). Coverage stays `Complete`; metrics and evidence match the same fixture with the echo lines removed; `records_unrecognized_inert` stays `0` because an allowlisted inert record is not counted as unrecognized.
- `token_count_without_usage.jsonl` has a `token_count` heartbeat (`info: null` beside `rate_limits`), a `token_count` whose usage objects hold only zero counts, one `token_count` with real usage, and a trailing `token_count` whose `last_token_usage` components are all zero beside a nonzero derived `total_tokens`, next to a `total_token_usage` that still carries the prior turn's real nonzero cumulative. `token_count_event` yields no event for any of the three usage-free shapes, so `is_usage_free_token_count` classifies them as recognized-eventless (measured: 514 heartbeats across 418 of 1,034 local rollouts, 1,355 zero-usage records across 366). Coverage stays `Complete`, `records_unusable` and `records_unrecognized_inert` are `0`, and only the third record's usage counts. A usage object with a key `codex_usage` does not read still fails closed.
- `inert_unknown_event.jsonl` has an `event_msg` with a made-up `payload.type` (`synthetic_progress`) carrying only ids, a `text`, and a duration. Coverage stays `Complete`, `records_unrecognized_inert` is `1`, and the `event_msg.synthetic_progress` discriminator is retained.
- `unknown_event_with_usage.jsonl` has an `event_msg` with a made-up type whose payload carries `last_token_usage`. Coverage is `Partial(UnrecognizedRecordType)`, the record is not inert, and the usage is not counted.
- `unknown_event_with_tool_shape.jsonl` has an unrecognized `response_item` payload with a nested `name` + `arguments` + `call_id`. Fails closed the same way as `unknown_event_with_usage.jsonl`.
