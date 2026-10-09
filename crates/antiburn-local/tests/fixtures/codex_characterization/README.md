# Codex rollout characterization

These fixtures are synthetic. They follow the public `openai/codex` rollout types at commit `e9a446d` and contain no captured session data.

## Persisted question and plan contract

`scope_records.jsonl` uses public producer commit
`e7637306bc9246a3e42e407cb94f96b7ed345e3e`. Its workspace version is `0.0.0`.
This pin defines an accepted shape, not a released version range. All values in
the fixture are synthetic. The following pinned public files define the contract:

- [Rollout persistence policy](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/rollout/src/policy.rs)
- [Rollout serialization](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/history/src/rollout_payload.rs)
- [Question argument specification](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/core/src/tools/handlers/request_user_input_spec.rs)
- [Question response types](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/protocol/src/request_user_input.rs)
- [Question handler and cancellation](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/core/src/tools/handlers/request_user_input.rs)
- [Retained answers and bounds](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/history/src/retained_context.rs)
- [Plan progress handler](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/core/src/tools/handlers/plan.rs)
- [Native completed Plan item](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/protocol/src/items.rs)
- [Proposed-plan delimiters](https://github.com/openai/codex/blob/e7637306bc9246a3e42e407cb94f96b7ed345e3e/codex-rs/utils/stream-parser/src/proposed_plan.rs)

The accepted question call is `response_item.function_call` with the exact plain
name `request_user_input`, `call_id`, and JSON-string `arguments`. Each question
has `id`, `header`, `question`, and nonempty options with `label` and `description`.
The result is `response_item.function_call_output` with the same `call_id` and a
JSON-string `output` containing `{answers:{qid:{answers:[strings]}}}`. The parser
preserves multiple strings, custom text, option descriptions, and question order.
It preserves the header in typed selected records. Matched results retain native
argument and output ranges with optional record IDs. A list of answers does not
prove a multi-select UI.

The exact cancellation error is
`request_user_input was cancelled before receiving a response`. It supplies no
answer. Other errors and malformed results have unknown status. Missing results
remain pending. Duplicate call IDs, unknown tools, namespaced calls, missing calls,
and malformed source gaps cannot supply submitted answers.

`retained_context.verified_answer` carries `turn_id`, `call_id`,
`questions:[{question,answer}]`, and optional `acceptance_order`. It is gated by
Guardian Approval and thread-owned context in this producer. Its question text
appends the selected option descriptions, and its answer joins nonempty responses
with newlines. Exact turn/call/content joins can suppress a later duplicate tool
answer. A later retained record with acceptance order keeps its typed answer and
order even when it matches an earlier tool result. Both original records retain
their private payload. Changed contents and unresolved identities remain separate evidence.
Retained records can arrive before the tool result. Empty bounded payloads mark
incomplete evidence. Compaction checkpoints retain bounded answer excerpts;
incomplete checkpoints degrade coverage and never reconstruct user messages or
replace the complete recorded scope. Checkpoint entries keep their native source
bindings and typed acceptance order.

Persisted answer records do not record human, automatic, or synthetic origin.
The parser sets `unknown_origin` for all of them, including retained answers.
It never treats these records as authoritative human approval.

`update_plan` arguments retain exact proposal/progress text. Completed steps and
`Plan updated` are not approval. Assistant `<proposed_plan>` text and native
`event_msg.item_completed` with `item.type: Plan`, `id`, and `text` retain recorded
plan versions as proposals. Actual ordinary user implementation requests retain
their original text and chronology. The parser does not infer a semantic approval
relationship. Live `RequestUserInput` and `PlanUpdate` events and app-server
schemas are not accepted substitutes for persisted rollout records.

Codex writes `{timestamp,type,payload}` JSONL under `~/.codex/sessions/YYYY/MM/DD/`. The rollout policy persists session metadata, turn context, selected response items, token counts, and compaction records. The writer opens a rollout and appends lines. The public rollout code exposes no compaction or in-place rewrite path for these JSONL files. Antiburn still uses a full source recheck because repository fixtures cannot prove external writer behavior. `state_5.sqlite` is a separate state store and is not a rollout source.

## Paginated completed-work contract

`paginated_completed.jsonl` is independently authored synthetic data. The accepted
producer is Codex 0.160.1 at `d27764b82f7118f674371e6d6e76271d9d606edb`.
The native VS Code capture establishes this recorded shape. The producer sources
define [typed items](https://github.com/openai/codex/blob/d27764b82f7118f674371e6d6e76271d9d606edb/codex-rs/protocol/src/items.rs),
[persistence policy](https://github.com/openai/codex/blob/d27764b82f7118f674371e6d6e76271d9d606edb/codex-rs/rollout/src/policy.rs),
and [recording](https://github.com/openai/codex/blob/d27764b82f7118f674371e6d6e76271d9d606edb/codex-rs/rollout/src/recorder.rs).
This is an accepted shape, not a historical release range.

- The header selects `history_mode=paginated`, `cli_version=0.160.1`, and a native
  session ID. Consecutive ordinals start at zero. Fork, parent-thread, history-base,
  rollback, and compaction shapes cannot establish the retained-root scope.
- The same pinned retained-message shape can establish a legacy retained root
  with `history_mode=legacy`. This mode does not require paginated ordinals.
  Its support comes from the pinned shared recorder and synthetic characterization;
  it is not a claim that the inspected VS Code capture used legacy history.
  Older messages without exact native identity and retained metadata cannot supply
  this proof. Legacy completed variants outside the characterized Plan contract
  remain unavailable for full-source admission.
- Only complete retained original messages with exact message/turn IDs,
  `content_item_kinds=[user.text]`, text-only content, and native retained revision
  supply `UserTextHistoryProof`. The proof concerns the current retained source.
  It does not prove that external deletion never occurred. Shared full-source
  admission must reject framing, loss, branch, and unknown-authority gaps.
- Typed UserMessage text must match the preceding raw projection within the same
  thread and turn. Picker name/path remains validated selection metadata; the
  parser does not publish a second user-text action for it. The selected document
  uses `skills.selected_skill_instructions` and carries `JevSelectedSkillProof`
  with source/session/message identity, name/location, normalization revision 1,
  and an exact full-text `UserMessage` range in its bindings. Its status is
  `DocumentSelected`, its authority remains unknown, and it has no human-history
  proof. Neither selection record is human authorization or successful skill
  execution. Assistant completed-message projections must match
  the next raw response by exact ID, turn, and text.
- CommandExecution and FileChange retain separate work/result parts with the same
  native item ID. An outer exec script can contain several operations. Its call ID
  cannot identify any individual nested item, so the parser makes no positional
  join. Outer exec content and nested output echoes do not duplicate performed work.
- Commands accept the captured three-element shell argv shape. Success requires
  `status=completed` and `exit_code=0`; failure requires `status=failed` and a
  nonzero code. Missing or conflicting fields remain unknown. Output remains
  recorded text. Duplicate item IDs with changed turn or contents degrade coverage.
- FileChange accepts native add/content, update/unified_diff/optional move_path,
  and delete shapes. Its status is operation status, not human approval.
- Observed reads accept only the characterized single-file
  `nl -ba PATH | sed -n 'START,ENDp'` shell slice with matching native parsed-command
  path. Returned consecutive numbered stdout lines prove only the observed range.
  Requested end, pipeline success, and producer parsed-command classification do
  not prove whole-file access. Missing, failed, clipped, empty, noncontiguous, and
  mismatched stdout do not prove an extent. File version remains unavailable.
- Output at the 64 KiB persistence bound or with conservative clipping markers
  remains truncated. Other shell read forms, attachments, native completed item
  variants outside this contract, and command argv forms remain unsupported.
  Each native identity map retains at most 4,096 identities. Overflow degrades
  coverage. Replayed raw message IDs do not repeat human authority; changed
  payloads under the same ID remain ambiguous and degrade coverage.

These are parser contracts. Desktop scope, result projection, and skill-use
admission need the shared integration gates before they become product support.

### Non-authorizing environment context

`environment_context.jsonl` is synthetic. The observed native capture and the
pinned [environment fragment producer](https://github.com/openai/codex/blob/d27764b82f7118f674371e6d6e76271d9d606edb/codex-rs/core/src/context/world_state/environment.rs)
and [filesystem renderer](https://github.com/openai/codex/blob/d27764b82f7118f674371e6d6e76271d9d606edb/codex-rs/core/src/context/environment_context.rs)
establish this contract. The producer emits the user-role fragment with native
`content_item_kinds=[environments.environment_context]`, not `user.text`.

The adapter accepts the characterized single-local-environment snapshot: one
`input_text` block, exact native message/turn identities and recorded creation
time, full environment markers, ordered cwd/shell/date/timezone lines, and a
managed restricted filesystem with bounded read/write path or special entries.
XML text uses the producer's five named escapes. The adapter keeps the text at
unknown authority and attaches a full native byte range, text digest, producer
pin, source/session/message identity, completeness, and normalization revision 1.
It adds no human-history proof, approval, or skill-use claim.

The shared normalizer binds this proof to the published action reference, source
key, thread, scope, and exact text. The factory must match the normalized fact to
its session key. It does not parse native markup. Unqualified lookalikes,
inherited or clipped content, unsupported producer versions, multi-environment
and delta shapes, other permission profiles, injected markup, and changed
bindings or bytes cannot justify omission from human authorization history.

## Legacy capability matrix

| Capability | State | Extracted source fact |
| --- | --- | --- |
| Request context tokens | yes | `token_count.info.last_token_usage.input_tokens` and cached input |
| Cache-write tokens | yes | `token_count.info.last_token_usage.cache_write_input_tokens` when the rollout emits it |
| Timestamps and order | yes | Every public `RolloutLine` has `timestamp` |
| Tool invocations | yes | Persisted response tool-call variants; unknown and paginated variants degrade coverage |
| Skill attribution | observed subset | Selected full skill documents can prove injection and invocation, not a full historical inventory |
| MCP attribution | observed subset | Completed `tool_search_output` namespace records can prove named server exposure, not a full historical inventory |
| Tool definitions | yes | The harness version and model resolve against the embedded built-in tool catalogue |
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
| Quota incidents | yes | `event_msg`/`task_complete` with a non-null `error`; only `rate_limit_exceeded` and `usage_limit_exceeded` are mapped |
| Provider incidents | yes | `event_msg`/`task_complete` with a non-null `error`; `server_overloaded` and `internal_server_error` map to `Capacity`/`ServerError`, and the four transport struct variants map by `http_status_code` to `ServerError` (5xx) or `Connection` (absent or null) |
| Harness version | yes | `session_meta.payload.cli_version` |

Sessions Over Depth, Model Overthinking, Overpowered Subagents, Old Model Usage,
Fast-Mode Overuse, and Cache Churn have the required capability prerequisites.
MCP, built-in-tool, and skill evidence supports only observed-subset findings.
No resource reader proves a full historical inventory, so M/B/K cannot report a
session-wide clean result.

An unrecognized `(type, payload.type)` combination no longer fails coverage closed by default (#229 parity). `is_inert_codex_record` proves a record structurally inert — no usage, model, effort, service tier, role, tool-shaped, or compaction-shaped keys at the depth its readers cover — before the record is skipped with `Complete` coverage and its discriminator retained. A record that fails the proof stays `Unusable(UnrecognizedRecordType)`, exactly as before. `event_msg`/`item_completed` and top-level `inter_agent_communication_metadata` are allowlisted as proven echoes of records this adapter already models (measured against 1,034 local rollouts: no sampled record of either family carried usage, a model, or an effort) and pass the lighter check that only reads the record's root and root `payload` object; every other unrecognized family is proved inert one record at a time by the strict, any-depth check. `session_meta`, `turn_context`, `world_state`, and the pre-existing `event_msg` housekeeping payloads bypass the structural check entirely: their own evidence-bearing fields (`turn_context.model`/`.effort`, `thread_settings_applied`'s `service_tier`) are read by `observe_model_and_effort` / `service_tier_speed` on every record, before classification runs, so nothing about them is left unproven.

Codex multi-agent ("collab") sessions add a tenth `event_msg` family: `collab_agent_spawn_begin`/`_end`, `collab_agent_interaction_begin`/`_end`, `collab_waiting_begin`/`_end`, `collab_close_begin`/`_end`, and `collab_resume_begin`/`_end`. Each pairs a begin and an end record around one step of an inter-agent call, and each field repeats data the session's own `spawn_agent` function call already carries. A verified read of the public `openai/codex` protocol source's `EventMsg` enum (not a sample) found no usage, token, or billing field on any of the ten payload structs, so all ten are allowlisted the same way `session_meta`/`turn_context` are and bypass the structural check entirely. `collab_agent_spawn_begin`/`_end` do carry `model` and `reasoning_effort`, naming the spawned agent's own configuration rather than billing evidence; those two fields are read by `observe_model_and_effort` before classification runs, same as `turn_context.model`/`.effort`, so nothing about them is left unproven either. The other eight variants carry no evidence-bearing field at all.

## Coverage cases

- `records_all_kinds.jsonl` covers supported legacy records and duplicate compaction markers.
- `malformed_between_valid.jsonl` keeps valid neighbors and reports partial coverage.
- `unrecognized_type.jsonl` carries a single `event_msg`/`item_completed` record with a made-up `item.type`. Since `item_completed` is now allowlisted and structurally inert (the unknown `item.type` sits inside the echoed `item`, past the light check's depth), this now reports `Complete` coverage and records no discriminator; see `inert_unknown_event.jsonl` for a genuinely unrecognized `event_msg` payload type instead.
- `absent_model_and_effort.jsonl` reports incomplete attribution instead of a clean model result.
- `resolved_fork.jsonl` excludes replayed parent token counts and keeps child-owned usage.
- `reverted_fork.jsonl` models a legacy top-level revert: the copied parent metadata and prefix share one writer timestamp, while the child records start at a later timestamp. The adapter excludes the replayed parent usage and keeps the child's task-start context window.
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
- `task_complete_errors.jsonl` has one ordinary turn, then sixteen `task_complete` shapes: a clean turn, the four originally-reviewed `codex_error_info` codes (`server_overloaded`, `rate_limit_exceeded`, `usage_limit_exceeded`, and `http_connection_failed` with a `503` status, which now maps to `ServerError`), the unmapped `"other"` code, an `error` with no `codex_error_info`, a mapped code with no top-level `timestamp`, `internal_server_error`, `http_connection_failed` with a `502` status, `response_stream_connection_failed` with a `null` status, `response_stream_disconnected` with no `http_status_code` key, `response_too_many_failed_attempts` with a `429` status (ignored, ambiguous layer), `http_connection_failed` with a string `"503"` status (ignored, not an integer), `context_window_exceeded` (ignored, the user's own context), and `active_turn_not_steerable` (ignored). `server_overloaded` and `internal_server_error` become `Capacity`/`ServerError` `ProviderIncident`s; `rate_limit_exceeded`/`usage_limit_exceeded` become `QuotaIncident`s; the four transport struct variants with a `5xx` or absent/`null` status become `ServerError`/`Connection` `ProviderIncident`s; coverage stays `Complete` and `records_unrecognized_inert` stays `0`.
- `eventless_protocol_records.jsonl` covers official `web_search_begin` / `web_search_end` lifecycle events and persisted inter-agent communication. Their protocol fields carry no usage, model, effort, or tool-call evidence, so coverage stays `Complete` without an inert-record diagnostic.

### #229-parity cases (no golden; exercised by dedicated assertions in `codex_characterization.rs`)

- `response_item_native_shapes.jsonl` asserts message authority, string-encoded arguments, nested object arguments, and exact output joins for function, custom, tool-search, and MCP calls. The content sink excludes arguments for `local_shell_call`, `tool_search_call`, and `web_search_call`. Recorded tool-search and web-search outputs remain available. The encrypted compaction payload is a boundary marker, not captured message text.
- `item_completed_echo.jsonl` has an `event_msg`/`item_completed` record after each of a user message, an assistant message, and a function call (`item.type` `UserMessage` / `AgentMessage` / `CommandExecution`, the last with `command`, `exit_code`, `stdout`, `aggregated_output` keys). Coverage stays `Complete`; metrics and evidence match the same fixture with the echo lines removed; `records_unrecognized_inert` stays `0` because an allowlisted inert record is not counted as unrecognized.
- `token_count_without_usage.jsonl` has a `token_count` heartbeat (`info: null` beside `rate_limits`), a `token_count` whose usage objects hold only zero counts, one `token_count` with real usage, and a trailing `token_count` whose `last_token_usage` components are all zero beside a nonzero derived `total_tokens`, next to a `total_token_usage` that still carries the prior turn's real nonzero cumulative. `token_count_event` yields no event for any of the three usage-free shapes, so `is_usage_free_token_count` classifies them as recognized-eventless (measured: 514 heartbeats across 418 of 1,034 local rollouts, 1,355 zero-usage records across 366). Coverage stays `Complete`, `records_unusable` and `records_unrecognized_inert` are `0`, and only the third record's usage counts. A usage object with a key `codex_usage` does not read still fails closed.
- `inert_unknown_event.jsonl` has an `event_msg` with a made-up `payload.type` (`synthetic_progress`) carrying only ids, a `text`, and a duration. Coverage stays `Complete`, `records_unrecognized_inert` is `1`, and the `event_msg.synthetic_progress` discriminator is retained.
- `unknown_event_with_usage.jsonl` has an `event_msg` with a made-up type whose payload carries `last_token_usage`. Coverage is `Partial(UnrecognizedRecordType)`, the record is not inert, and the usage is not counted.
- `unknown_event_with_tool_shape.jsonl` has an unrecognized `response_item` payload with a nested `name` + `arguments` + `call_id`. Fails closed the same way as `unknown_event_with_usage.jsonl`.
