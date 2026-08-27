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
| Fast tier | no | No speed mode field |
| Service tier | no | The adapter does not extract the sparse setting |
| Subagent relationships | no | Discovery can relate files, but this evidence adapter does not publish the relation |
| Subagent models | no | A subagent activity record does not carry its model |
| Compaction boundaries | yes | Top-level `compacted` and legacy `context_compacted` |
| Thread identity | no | Records have no Claude-style record and parent identities |
| Quota incidents | no | The evidence sink has no incident ingestion path in this slice |
| Harness version | no | The evidence sink has no version ingestion path in this slice |

Only Model Overthinking and Old Model Usage have all capability prerequisites. Every other detector remains not assessed.

## Coverage cases

- `records_all_kinds.jsonl` covers supported legacy records and duplicate compaction markers.
- `malformed_between_valid.jsonl` keeps valid neighbors and reports partial coverage.
- `unrecognized_type.jsonl` records a fixed technical discriminator and reports partial coverage.
- `absent_model_and_effort.jsonl` reports incomplete attribution instead of a clean model result.
- `resolved_fork.jsonl` excludes replayed parent token counts and keeps child-owned usage.
- `unresolved_fork.jsonl` excludes replayed usage and reports incomplete attribution.
- `incomplete_final_record.jsonl` models an active writer stopped inside its final line.
